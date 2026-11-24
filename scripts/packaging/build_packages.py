#!/usr/bin/env python3
"""Build Linux .deb, .rpm, and .AppImage (Type 2) packages from dist/linux/."""

from __future__ import annotations

import hashlib
import io
import lzma
import os
import shutil
import stat
import struct
import subprocess
import tarfile
import time
from dataclasses import dataclass
from pathlib import Path

ROOT_DIR = Path(__file__).resolve().parents[2]
DIST_DIR = ROOT_DIR / "dist" / "linux"
STAGE_ROOT = ROOT_DIR / "target" / "linux-pkg"

PKG_NAME = "me-edfork-workspace"
APP_ID = "me.edfork.workspace"
ARCH_DEB = "amd64"
ARCH_RPM = "x86_64"
ARCH_APPIMAGE = "x86_64"
SUMMARY_EN = "Secure LAN clipboard sync and file transfer between Linux and Android"
DESCRIPTION_TEXT = (
    "Universe Workstation connects Linux desktops and rooted Android devices over "
    "your local network with Ed25519 signed discovery, 6-digit SAS pairing, pinned "
    "TLS 1.3 mutual authentication, lossless text & image clipboard sync, and "
    "BLAKE3-verified streaming file transfers."
)


def load_version_props() -> dict[str, str]:
    props: dict[str, str] = {}
    for raw in (ROOT_DIR / "version.properties").read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        k, v = line.split("=", 1)
        props[k.strip()] = v.strip()
    return props


def make_system_desktop_bytes() -> bytes:
    return (
        "[Desktop Entry]\n"
        "Type=Application\n"
        "Name=寰宇工作台\n"
        "Name[en]=Universe Workstation\n"
        "GenericName=LAN Device Bridge\n"
        "Comment=Secure LAN clipboard sync and file transfer between Linux and Android\n"
        f"TryExec=/usr/bin/{PKG_NAME}\n"
        f"Exec=/usr/bin/{PKG_NAME}\n"
        f"Icon={APP_ID}\n"
        "Terminal=false\n"
        "Categories=Network;Utility;\n"
        "StartupNotify=false\n"
        f"StartupWMClass={APP_ID}\n"
    ).encode("utf-8")


def make_system_service_bytes() -> bytes:
    return (
        "[Unit]\n"
        f"Description=Universe Workstation ({APP_ID}) LAN Bridge Daemon\n"
        "After=graphical-session.target\n"
        "PartOf=graphical-session.target\n\n"
        "[Service]\n"
        "Type=simple\n"
        f"ExecStart=/usr/bin/{PKG_NAME} --silent\n"
        "Restart=on-failure\n"
        "RestartSec=3\n\n"
        "[Install]\n"
        "WantedBy=graphical-session.target\n"
    ).encode("utf-8")


def generate_icons(icon_src: Path, work_dir: Path) -> dict[int, bytes]:
    icons: dict[int, bytes] = {}
    for sz in (512, 256, 128, 64, 48):
        if sz == 512:
            icons[sz] = icon_src.read_bytes()
            continue
        out_png = work_dir / f"icon_{sz}.png"
        try:
            from PIL import Image

            with Image.open(icon_src) as im:
                im.resize((sz, sz), Image.Resampling.LANCZOS).save(out_png, "PNG")
            icons[sz] = out_png.read_bytes()
        except Exception:
            if shutil.which("magick"):
                subprocess.run(
                    ["magick", str(icon_src), "-resize", f"{sz}x{sz}", str(out_png)],
                    check=True,
                )
                icons[sz] = out_png.read_bytes()
            else:
                icons[sz] = icon_src.read_bytes()
    return icons


def prepare_stripped_binaries(work_dir: Path) -> dict[str, bytes]:
    bin_dir = work_dir / "stripped"
    bin_dir.mkdir(parents=True, exist_ok=True)
    names = ("me-edfork-workspace", "workspace-share", "libnautilus_workspace.so")
    out: dict[str, bytes] = {}
    for name in names:
        src = DIST_DIR / name
        dst = bin_dir / name
        shutil.copy2(src, dst)
        if shutil.which("strip"):
            subprocess.run(["strip", "--strip-unneeded", str(dst)], check=True)
        out[name] = dst.read_bytes()
    return out


# ---------------- .deb Builder ----------------

def _tar_add_dir(tf: tarfile.TarFile, arcpath: str, mtime: int) -> None:
    ti = tarfile.TarInfo(name=arcpath)
    ti.type = tarfile.DIRTYPE
    ti.mode = 0o755
    ti.uid = 0
    ti.gid = 0
    ti.uname = "root"
    ti.gname = "root"
    ti.mtime = mtime
    tf.addfile(ti)


def _tar_add_file(tf: tarfile.TarFile, arcpath: str, data: bytes, mode: int, mtime: int) -> None:
    ti = tarfile.TarInfo(name=arcpath)
    ti.type = tarfile.REGTYPE
    ti.mode = mode
    ti.size = len(data)
    ti.uid = 0
    ti.gid = 0
    ti.uname = "root"
    ti.gname = "root"
    ti.mtime = mtime
    tf.addfile(ti, io.BytesIO(data))


def build_tar_xz(entries: list[tuple[str, bytes, int]], mtime: int) -> bytes:
    buf = io.BytesIO()
    with lzma.open(buf, mode="wb", format=lzma.FORMAT_XZ, preset=6) as xzf:
        with tarfile.open(fileobj=xzf, mode="w", format=tarfile.GNU_FORMAT) as tf:
            seen_dirs = {"."}
            _tar_add_dir(tf, ".", mtime)
            for rel_path, data, mode in sorted(entries, key=lambda x: x[0]):
                clean = rel_path.lstrip("/")
                parts = clean.split("/")
                for i in range(1, len(parts)):
                    d = "./" + "/".join(parts[:i])
                    if d not in seen_dirs:
                        seen_dirs.add(d)
                        _tar_add_dir(tf, d, mtime)
                _tar_add_file(tf, "./" + clean, data, mode, mtime)
    return buf.getvalue()


def write_ar_archive(out_path: Path, members: list[tuple[str, bytes]], mtime: int) -> None:
    out = bytearray(b"!<arch>\n")
    for name, data in members:
        assert len(name) <= 16
        hdr = (
            f"{name:<16}"
            f"{mtime:<12}"
            f"{0:<6}"
            f"{0:<6}"
            f"{'100644':<8}"
            f"{len(data):<10}"
            "`\n"
        ).encode("ascii")
        assert len(hdr) == 60
        out.extend(hdr)
        out.extend(data)
        if len(data) % 2 == 1:
            out.append(ord("\n"))
    out_path.write_bytes(bytes(out))


def build_deb(
    version_name: str,
    bins: dict[str, bytes],
    icons: dict[int, bytes],
    desktop_bytes: bytes,
    service_bytes: bytes,
    appdata_bytes: bytes,
    copyright_bytes: bytes,
    notices_bytes: bytes,
    mtime: int,
) -> Path:
    data_entries: list[tuple[str, bytes, int]] = [
        (f"usr/bin/{PKG_NAME}", bins[PKG_NAME], 0o755),
        ("usr/bin/workspace-share", bins["workspace-share"], 0o755),
        (
            "usr/lib/x86_64-linux-gnu/nautilus/extensions-4/libnautilus_workspace.so",
            bins["libnautilus_workspace.so"],
            0o644,
        ),
        (f"usr/lib/systemd/user/{PKG_NAME}.service", service_bytes, 0o644),
        (f"usr/share/applications/{APP_ID}.desktop", desktop_bytes, 0o644),
        (f"usr/share/metainfo/{APP_ID}.appdata.xml", appdata_bytes, 0o644),
        (f"usr/share/doc/{PKG_NAME}/copyright", copyright_bytes, 0o644),
        (f"usr/share/doc/{PKG_NAME}/THIRD_PARTY_NOTICES.md", notices_bytes, 0o644),
    ]
    for sz, png_bytes in icons.items():
        data_entries.append((f"usr/share/icons/hicolor/{sz}x{sz}/apps/{APP_ID}.png", png_bytes, 0o644))

    installed_kb = max(1, (sum(len(d) for _, d, _ in data_entries) + 1023) // 1024)
    md5_lines = [
        f"{hashlib.md5(d).hexdigest()}  {p.lstrip('/')}"
        for p, d, _ in sorted(data_entries, key=lambda x: x[0])
    ]
    md5sums_bytes = ("\n".join(md5_lines) + "\n").encode("utf-8")

    control_text = (
        f"Package: {PKG_NAME}\n"
        f"Version: {version_name}\n"
        f"Architecture: {ARCH_DEB}\n"
        "Maintainer: edfork <https://github.com/edfork/FileShareBridge>\n"
        f"Installed-Size: {installed_kb}\n"
        "Section: net\n"
        "Priority: optional\n"
        "Depends: libc6 (>= 2.39), libgtk-4-1 (>= 4.14), libadwaita-1-0 (>= 1.4), "
        "libglib2.0-0t64 (>= 2.74) | libglib2.0-0 (>= 2.74), libcairo2 (>= 1.16)\n"
        "Recommends: wl-clipboard | xclip, libnautilus-extension4 (>= 43.0)\n"
        f"Description: {SUMMARY_EN}\n"
        f" {DESCRIPTION_TEXT}\n"
        " .\n"
        " Core components are licensed under MIT; the Nautilus extension plugin is\n"
        " separately licensed under GPL-3.0-or-later.\n"
    ).encode("utf-8")

    postinst = (
        "#!/bin/sh\n"
        "set -e\n"
        "if command -v update-desktop-database >/dev/null 2>&1; then\n"
        "  update-desktop-database -q /usr/share/applications || true\n"
        "fi\n"
        "if command -v gtk-update-icon-cache >/dev/null 2>&1; then\n"
        "  gtk-update-icon-cache -f -t -q /usr/share/icons/hicolor || true\n"
        "fi\n"
        "if command -v ldconfig >/dev/null 2>&1; then\n"
        "  ldconfig || true\n"
        "fi\n"
        "exit 0\n"
    ).encode("utf-8")

    postrm = (
        "#!/bin/sh\n"
        "set -e\n"
        "if command -v update-desktop-database >/dev/null 2>&1; then\n"
        "  update-desktop-database -q /usr/share/applications || true\n"
        "fi\n"
        "if command -v gtk-update-icon-cache >/dev/null 2>&1; then\n"
        "  gtk-update-icon-cache -f -t -q /usr/share/icons/hicolor || true\n"
        "fi\n"
        "exit 0\n"
    ).encode("utf-8")

    control_entries = [
        ("control", control_text, 0o644),
        ("md5sums", md5sums_bytes, 0o644),
        ("postinst", postinst, 0o755),
        ("postrm", postrm, 0o755),
    ]

    control_txz = build_tar_xz(control_entries, mtime)
    data_txz = build_tar_xz(data_entries, mtime)

    deb_path = DIST_DIR / f"{PKG_NAME}_{version_name}_{ARCH_DEB}.deb"
    write_ar_archive(
        deb_path,
        [
            ("debian-binary", b"2.0\n"),
            ("control.tar.xz", control_txz),
            ("data.tar.xz", data_txz),
        ],
        mtime,
    )
    deb_path.chmod(0o644)
    return deb_path


# ---------------- .rpm Builder ----------------

RPM_TYPE_INT16 = 2
RPM_TYPE_INT32 = 3
RPM_TYPE_STRING = 6
RPM_TYPE_BIN = 7
RPM_TYPE_STRING_ARRAY = 8
RPM_TYPE_I18NSTRING = 9

RPMSENSE_LESS = 1 << 1
RPMSENSE_GREATER = 1 << 2
RPMSENSE_EQUAL = 1 << 3
RPMSENSE_RPMLIB = 1 << 24


@dataclass
class RpmFileEntry:
    path: str
    data: bytes | None  # None = directory
    mode: int
    flags: int = 0  # 2 = doc, 128 = license


def build_cpio_newc(entries: list[RpmFileEntry], mtime: int) -> tuple[bytes, dict[str, int]]:
    buf = bytearray()
    ino_map: dict[str, int] = {}
    ino = 1

    def emit(name_str: str, mode: int, data: bytes, nlink: int) -> None:
        name_bytes = name_str.encode("utf-8") + b"\x00"
        hdr = (
            f"070701"
            f"{ino:08x}"
            f"{mode:08x}"
            f"{0:08x}"
            f"{0:08x}"
            f"{nlink:08x}"
            f"{mtime:08x}"
            f"{len(data):08x}"
            f"{0:08x}"
            f"{0:08x}"
            f"{0:08x}"
            f"{0:08x}"
            f"{len(name_bytes):08x}"
            f"{0:08x}"
        ).encode("ascii")
        assert len(hdr) == 110
        buf.extend(hdr)
        buf.extend(name_bytes)
        while len(buf) % 4 != 0:
            buf.append(0)
        buf.extend(data)
        while len(buf) % 4 != 0:
            buf.append(0)

    for e in entries:
        ino_map[e.path] = ino
        cpio_name = "." + e.path
        if e.data is None:
            emit(cpio_name, stat.S_IFDIR | e.mode, b"", 2)
        else:
            emit(cpio_name, stat.S_IFREG | e.mode, e.data, 1)
        ino += 1

    # Trailer
    ino_trailer = ino
    name_bytes = b"TRAILER!!!\x00"
    hdr = (
        f"070701"
        f"{ino_trailer:08x}"
        f"{0:08x}{0:08x}{0:08x}{1:08x}{0:08x}{0:08x}"
        f"{0:08x}{0:08x}{0:08x}{0:08x}{len(name_bytes):08x}{0:08x}"
    ).encode("ascii")
    buf.extend(hdr)
    buf.extend(name_bytes)
    while len(buf) % 4 != 0:
        buf.append(0)

    return bytes(buf), ino_map


def encode_rpm_header(
    tags: list[tuple[int, int, object]],
    region_tag: int,
    pad8: bool = False,
) -> bytes:
    """Serialize an RPM header with an immutable region trailer."""
    store = bytearray()
    raw_entries: list[tuple[int, int, int, int]] = []

    def align_store(align: int) -> None:
        while len(store) % align != 0:
            store.append(0)

    # Reserve region tag as first slot; its trailer is appended last.
    for tag, rtype, val in sorted(tags, key=lambda item: item[0]):
        if rtype == RPM_TYPE_INT16:
            vals = list(val)  # type: ignore[arg-type]
            align_store(2)
            off = len(store)
            for v in vals:
                store.extend(struct.pack(">H", v))
            raw_entries.append((tag, rtype, off, len(vals)))
        elif rtype == RPM_TYPE_INT32:
            vals = list(val)  # type: ignore[arg-type]
            align_store(4)
            off = len(store)
            for v in vals:
                store.extend(struct.pack(">I", v))
            raw_entries.append((tag, rtype, off, len(vals)))
        elif rtype == RPM_TYPE_STRING:
            off = len(store)
            store.extend(str(val).encode("utf-8") + b"\x00")
            raw_entries.append((tag, rtype, off, 1))
        elif rtype == RPM_TYPE_BIN:
            b = bytes(val)  # type: ignore[arg-type]
            off = len(store)
            store.extend(b)
            raw_entries.append((tag, rtype, off, len(b)))
        elif rtype in (RPM_TYPE_STRING_ARRAY, RPM_TYPE_I18NSTRING):
            strs = list(val)  # type: ignore[arg-type]
            off = len(store)
            for s in strs:
                store.extend(str(s).encode("utf-8") + b"\x00")
            raw_entries.append((tag, rtype, off, len(strs)))
        else:
            raise ValueError(f"Unsupported RPM type {rtype}")

    total_entries = len(raw_entries) + 1
    trailer_off = len(store)
    trailer = struct.pack(">IIii", region_tag, RPM_TYPE_BIN, -(total_entries * 16), 16)
    store.extend(trailer)

    all_entries = [(region_tag, RPM_TYPE_BIN, trailer_off, 16)] + raw_entries
    index_bytes = bytearray()
    for tag, rtype, off, count in all_entries:
        index_bytes.extend(struct.pack(">IIII", tag, rtype, off, count))

    header = bytearray(b"\x8e\xad\xe8\x01\x00\x00\x00\x00")
    header.extend(struct.pack(">II", len(all_entries), len(store)))
    header.extend(index_bytes)
    header.extend(store)
    if pad8:
        while (len(header) % 8) != 0:
            header.append(0)
    return bytes(header)


def build_rpm(
    version_name: str,
    bins: dict[str, bytes],
    icons: dict[int, bytes],
    desktop_bytes: bytes,
    service_bytes: bytes,
    appdata_bytes: bytes,
    mit_license_bytes: bytes,
    gpl_license_bytes: bytes,
    notices_bytes: bytes,
    mtime: int,
) -> Path:
    rpm_ver = version_name.replace("-", "_")
    rpm_rel = "1"
    nvra = f"{PKG_NAME}-{rpm_ver}-{rpm_rel}.{ARCH_RPM}"

    owned_dirs = [
        f"/usr/lib/systemd/user",
        f"/usr/lib64/nautilus",
        f"/usr/lib64/nautilus/extensions-4",
        f"/usr/share/doc/{PKG_NAME}",
        f"/usr/share/licenses/{PKG_NAME}",
    ]
    file_entries: list[RpmFileEntry] = [RpmFileEntry(d, None, 0o755) for d in owned_dirs]
    file_entries.extend([
        RpmFileEntry(f"/usr/bin/{PKG_NAME}", bins[PKG_NAME], 0o755),
        RpmFileEntry("/usr/bin/workspace-share", bins["workspace-share"], 0o755),
        RpmFileEntry(
            "/usr/lib64/nautilus/extensions-4/libnautilus_workspace.so",
            bins["libnautilus_workspace.so"],
            0o755,
        ),
        RpmFileEntry(f"/usr/lib/systemd/user/{PKG_NAME}.service", service_bytes, 0o644),
        RpmFileEntry(f"/usr/share/applications/{APP_ID}.desktop", desktop_bytes, 0o644),
        RpmFileEntry(f"/usr/share/metainfo/{APP_ID}.appdata.xml", appdata_bytes, 0o644),
        RpmFileEntry(f"/usr/share/doc/{PKG_NAME}/THIRD_PARTY_NOTICES.md", notices_bytes, 0o644, flags=2),
        RpmFileEntry(f"/usr/share/licenses/{PKG_NAME}/LICENSE", mit_license_bytes, 0o644, flags=128),
        RpmFileEntry(
            f"/usr/share/licenses/{PKG_NAME}/LICENSE-nautilus-extension-GPL-3.0.txt",
            gpl_license_bytes,
            0o644,
            flags=128,
        ),
    ])
    for sz, png_bytes in icons.items():
        file_entries.append(
            RpmFileEntry(f"/usr/share/icons/hicolor/{sz}x{sz}/apps/{APP_ID}.png", png_bytes, 0o644)
        )

    file_entries.sort(key=lambda e: e.path)

    cpio_raw, ino_map = build_cpio_newc(file_entries, mtime)
    payload_xz = lzma.compress(cpio_raw, format=lzma.FORMAT_XZ, preset=6)
    payload_digest_hex = hashlib.sha256(payload_xz).hexdigest()

    dirnames_list: list[str] = []
    dirname_idx: dict[str, int] = {}
    dirindexes: list[int] = []
    basenames: list[str] = []
    filesizes: list[int] = []
    filemodes: list[int] = []
    filerdevs: list[int] = []
    filemtimes: list[int] = []
    filedigests: list[str] = []
    filelinktos: list[str] = []
    fileflags: list[int] = []
    fileusers: list[str] = []
    filegroups: list[str] = []
    fileinodes: list[int] = []
    filelangs: list[str] = []
    total_size = 0

    for e in file_entries:
        parent, base = e.path.rsplit("/", 1)
        dname = (parent + "/") if parent else "/"
        if dname not in dirname_idx:
            dirname_idx[dname] = len(dirnames_list)
            dirnames_list.append(dname)
        dirindexes.append(dirname_idx[dname])
        basenames.append(base)
        if e.data is None:
            filesizes.append(4096)
            filemodes.append(stat.S_IFDIR | e.mode)
            filedigests.append("")
        else:
            sz = len(e.data)
            total_size += sz
            filesizes.append(sz)
            filemodes.append(stat.S_IFREG | e.mode)
            filedigests.append(hashlib.sha256(e.data).hexdigest())
        filerdevs.append(0)
        filemtimes.append(mtime)
        filelinktos.append("")
        fileflags.append(e.flags)
        fileusers.append("root")
        filegroups.append("root")
        fileinodes.append(ino_map[e.path])
        filelangs.append("")

    requires = [
        ("rpmlib(CompressedFileNames)", RPMSENSE_RPMLIB | RPMSENSE_LESS | RPMSENSE_EQUAL, "3.0.4-1"),
        ("rpmlib(FileDigests)", RPMSENSE_RPMLIB | RPMSENSE_LESS | RPMSENSE_EQUAL, "4.6.0-1"),
        ("rpmlib(PayloadFilesHavePrefix)", RPMSENSE_RPMLIB | RPMSENSE_LESS | RPMSENSE_EQUAL, "4.0-1"),
        ("rpmlib(PayloadIsXz)", RPMSENSE_RPMLIB | RPMSENSE_LESS | RPMSENSE_EQUAL, "5.2-1"),
        ("libadwaita-1.so.0()(64bit)", 0, ""),
        ("libgtk-4.so.1()(64bit)", 0, ""),
        ("libcairo.so.2()(64bit)", 0, ""),
        ("libgio-2.0.so.0()(64bit)", 0, ""),
        ("libgobject-2.0.so.0()(64bit)", 0, ""),
        ("libglib-2.0.so.0()(64bit)", 0, ""),
        ("libc.so.6(GLIBC_2.39)(64bit)", 0, ""),
    ]
    provides = [
        (PKG_NAME, RPMSENSE_EQUAL, f"{rpm_ver}-{rpm_rel}"),
        (f"{PKG_NAME}(x86-64)", RPMSENSE_EQUAL, f"{rpm_ver}-{rpm_rel}"),
    ]

    main_tags: list[tuple[int, int, object]] = [
        (100, RPM_TYPE_STRING_ARRAY, ["C"]),
        (1000, RPM_TYPE_STRING, PKG_NAME),
        (1001, RPM_TYPE_STRING, rpm_ver),
        (1002, RPM_TYPE_STRING, rpm_rel),
        (1004, RPM_TYPE_I18NSTRING, [SUMMARY_EN]),
        (1005, RPM_TYPE_I18NSTRING, [DESCRIPTION_TEXT]),
        (1006, RPM_TYPE_INT32, [mtime]),
        (1009, RPM_TYPE_INT32, [total_size]),
        (1014, RPM_TYPE_STRING, "MIT and GPL-3.0-or-later"),
        (1016, RPM_TYPE_I18NSTRING, ["Applications/Internet"]),
        (1021, RPM_TYPE_STRING, "linux"),
        (1022, RPM_TYPE_STRING, ARCH_RPM),
        (1028, RPM_TYPE_INT32, filesizes),
        (1030, RPM_TYPE_INT16, filemodes),
        (1033, RPM_TYPE_INT16, filerdevs),
        (1034, RPM_TYPE_INT32, filemtimes),
        (1035, RPM_TYPE_STRING_ARRAY, filedigests),
        (1036, RPM_TYPE_STRING_ARRAY, filelinktos),
        (1037, RPM_TYPE_INT32, fileflags),
        (1039, RPM_TYPE_STRING_ARRAY, fileusers),
        (1040, RPM_TYPE_STRING_ARRAY, filegroups),
        (1047, RPM_TYPE_STRING_ARRAY, [p[0] for p in provides]),
        (1048, RPM_TYPE_INT32, [r[1] for r in requires]),
        (1049, RPM_TYPE_STRING_ARRAY, [r[0] for r in requires]),
        (1050, RPM_TYPE_STRING_ARRAY, [r[2] for r in requires]),
        (1095, RPM_TYPE_INT32, fileinodes),
        (1096, RPM_TYPE_STRING_ARRAY, filelangs),
        (1112, RPM_TYPE_INT32, [p[1] for p in provides]),
        (1113, RPM_TYPE_STRING_ARRAY, [p[2] for p in provides]),
        (1116, RPM_TYPE_INT32, dirindexes),
        (1117, RPM_TYPE_STRING_ARRAY, basenames),
        (1118, RPM_TYPE_STRING_ARRAY, dirnames_list),
        (1124, RPM_TYPE_STRING, "cpio"),
        (1125, RPM_TYPE_STRING, "xz"),
        (1126, RPM_TYPE_STRING, "6"),
        (1132, RPM_TYPE_STRING, "x86_64-pc-linux-gnu"),
        (5011, RPM_TYPE_INT32, [8]),
        (5092, RPM_TYPE_STRING_ARRAY, [payload_digest_hex]),
        (5093, RPM_TYPE_INT32, [8]),
        (5097, RPM_TYPE_STRING_ARRAY, [payload_digest_hex]),
    ]
    main_header = encode_rpm_header(main_tags, region_tag=63, pad8=False)

    sig_tags: list[tuple[int, int, object]] = [
        (269, RPM_TYPE_STRING, hashlib.sha1(main_header).hexdigest()),
        (273, RPM_TYPE_STRING, hashlib.sha256(main_header).hexdigest()),
        (1000, RPM_TYPE_INT32, [len(main_header) + len(payload_xz)]),
        (1004, RPM_TYPE_BIN, hashlib.md5(main_header + payload_xz).digest()),
        (1007, RPM_TYPE_INT32, [len(cpio_raw)]),
    ]
    sig_header = encode_rpm_header(sig_tags, region_tag=62, pad8=True)

    lead = struct.pack(
        ">4sBBHH66sHH16s",
        b"\xed\xab\xee\xdb",
        3,
        0,
        0,
        1,
        nvra.encode("utf-8")[:65],
        1,
        5,
        b"\x00" * 16,
    )
    assert len(lead) == 96

    rpm_path = DIST_DIR / f"{nvra}.rpm"
    rpm_path.write_bytes(lead + sig_header + main_header + payload_xz)
    rpm_path.chmod(0o644)
    return rpm_path


# ---------------- .AppImage Builder ----------------

def build_appimage(
    version_name: str,
    bins: dict[str, bytes],
    icons: dict[int, bytes],
    appdata_bytes: bytes,
    mit_license_bytes: bytes,
    notices_bytes: bytes,
    work_dir: Path,
) -> Path:
    appdir = work_dir / "AppDir"
    if appdir.exists():
        shutil.rmtree(appdir)

    (appdir / "usr" / "bin").mkdir(parents=True)
    (appdir / "usr" / "share" / "applications").mkdir(parents=True)
    (appdir / "usr" / "share" / "metainfo").mkdir(parents=True)
    (appdir / "usr" / "share" / "licenses" / PKG_NAME).mkdir(parents=True)

    for sz, png_bytes in icons.items():
        idir = appdir / "usr" / "share" / "icons" / "hicolor" / f"{sz}x{sz}" / "apps"
        idir.mkdir(parents=True)
        (idir / f"{APP_ID}.png").write_bytes(png_bytes)

    for name in (PKG_NAME, "workspace-share"):
        p = appdir / "usr" / "bin" / name
        p.write_bytes(bins[name])
        p.chmod(0o755)

    desktop_appimage = (
        "[Desktop Entry]\n"
        "Type=Application\n"
        "Name=寰宇工作台\n"
        "Name[en]=Universe Workstation\n"
        "GenericName=LAN Device Bridge\n"
        "Comment=Secure LAN clipboard sync and file transfer between Linux and Android\n"
        f"Exec={PKG_NAME}\n"
        f"Icon={APP_ID}\n"
        "Terminal=false\n"
        "Categories=Network;Utility;\n"
        "StartupNotify=false\n"
        f"StartupWMClass={APP_ID}\n"
        f"X-AppImage-Version={version_name}\n"
    ).encode("utf-8")

    (appdir / f"{APP_ID}.desktop").write_bytes(desktop_appimage)
    (appdir / "usr" / "share" / "applications" / f"{APP_ID}.desktop").write_bytes(desktop_appimage)
    (appdir / "usr" / "share" / "metainfo" / f"{APP_ID}.appdata.xml").write_bytes(appdata_bytes)
    (appdir / "usr" / "share" / "licenses" / PKG_NAME / "LICENSE").write_bytes(mit_license_bytes)
    (appdir / "usr" / "share" / "licenses" / PKG_NAME / "THIRD_PARTY_NOTICES.md").write_bytes(notices_bytes)

    (appdir / f"{APP_ID}.png").write_bytes(icons[256])
    diricon = appdir / ".DirIcon"
    if diricon.exists() or diricon.is_symlink():
        diricon.unlink()
    diricon.symlink_to(f"{APP_ID}.png")

    apprun = (
        "#!/bin/sh\n"
        "set -eu\n"
        'HERE="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"\n'
        'export PATH="${HERE}/usr/bin:${PATH}"\n'
        'export WORKSPACE_SHARE_BIN="${HERE}/usr/bin/workspace-share"\n'
        'export XDG_DATA_DIRS="${HERE}/usr/share:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"\n'
        'if [ "${1:-}" = "--share" ]; then\n'
        "  shift\n"
        '  exec "${HERE}/usr/bin/workspace-share" "$@"\n'
        "fi\n"
        f'exec "${{HERE}}/usr/bin/{PKG_NAME}" "$@"\n'
    ).encode("utf-8")
    apprun_path = appdir / "AppRun"
    apprun_path.write_bytes(apprun)
    apprun_path.chmod(0o755)

    runtime_src = ROOT_DIR / "scripts" / "packaging" / "appimage_runtime.c"
    runtime_bin = work_dir / "appimage_runtime"
    subprocess.run(
        [
            "gcc",
            "-O2",
            "-s",
            "-Wall",
            "-Wextra",
            str(runtime_src),
            "-o",
            str(runtime_bin),
            "-lz",
        ],
        check=True,
    )

    rt_bytes = bytearray(runtime_bin.read_bytes())
    assert rt_bytes[:4] == b"\x7fELF", "Runtime binary is not ELF"
    # Stamp AppImage Type 2 magic ("AI\x02") in ELF e_ident[8..11]
    rt_bytes[8:12] = b"AI\x02\x00"
    pad = (-len(rt_bytes)) % 4096
    rt_bytes.extend(b"\x00" * pad)

    sqfs_path = work_dir / "appdir.sqfs"
    subprocess.run(
        [
            "mksquashfs",
            str(appdir),
            str(sqfs_path),
            "-comp",
            "gzip",
            "-b",
            "131072",
            "-no-fragments",
            "-no-xattrs",
            "-all-root",
            "-noappend",
            "-quiet",
        ],
        check=True,
    )

    appimage_path = DIST_DIR / f"Universe_Workstation-{version_name}-{ARCH_APPIMAGE}.AppImage"
    with open(appimage_path, "wb") as out:
        out.write(rt_bytes)
        out.write(sqfs_path.read_bytes())
    appimage_path.chmod(0o755)
    return appimage_path


def main() -> None:
    for required in (
        DIST_DIR / PKG_NAME,
        DIST_DIR / "workspace-share",
        DIST_DIR / "libnautilus_workspace.so",
    ):
        if not required.is_file():
            raise SystemExit(f"Missing build artifact: {required}. Run ./scripts/build-linux.sh first.")

    props = load_version_props()
    version_name = props["VERSION_NAME"]
    mtime = int(time.time())

    if STAGE_ROOT.exists():
        shutil.rmtree(STAGE_ROOT)
    STAGE_ROOT.mkdir(parents=True)

    bins = prepare_stripped_binaries(STAGE_ROOT)
    icons = generate_icons(ROOT_DIR / "assets" / f"{APP_ID}.png", STAGE_ROOT)
    desktop_bytes = make_system_desktop_bytes()
    service_bytes = make_system_service_bytes()
    appdata_bytes = (ROOT_DIR / "assets" / f"{APP_ID}.appdata.xml").read_bytes()
    mit_license_bytes = (ROOT_DIR / "LICENSE").read_bytes()
    gpl_license_bytes = (ROOT_DIR / "crates" / "workspace-nautilus" / "LICENSE").read_bytes()
    notices_bytes = (ROOT_DIR / "THIRD_PARTY_NOTICES.md").read_bytes()

    copyright_bytes = (
        "Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/\n"
        f"Upstream-Name: Universe Workstation ({PKG_NAME})\n"
        "Source: https://github.com/edfork/FileShareBridge\n\n"
        "Files: usr/lib/x86_64-linux-gnu/nautilus/extensions-4/libnautilus_workspace.so\n"
        "Copyright: 2026 edfork\n"
        "License: GPL-3.0-or-later\n\n"
        "Files: *\n"
        "Copyright: 2026 edfork\n"
        "License: MIT\n\n"
    ).encode("utf-8") + mit_license_bytes + b"\n" + ("=" * 72 + "\n").encode("utf-8") + gpl_license_bytes

    deb_path = build_deb(
        version_name,
        bins,
        icons,
        desktop_bytes,
        service_bytes,
        appdata_bytes,
        copyright_bytes,
        notices_bytes,
        mtime,
    )
    rpm_path = build_rpm(
        version_name,
        bins,
        icons,
        desktop_bytes,
        service_bytes,
        appdata_bytes,
        mit_license_bytes,
        gpl_license_bytes,
        notices_bytes,
        mtime,
    )
    appimage_path = build_appimage(
        version_name,
        bins,
        icons,
        appdata_bytes,
        mit_license_bytes,
        notices_bytes,
        STAGE_ROOT,
    )

    print("Created Linux packages:")
    for p in (deb_path, rpm_path, appimage_path):
        print(f"  {p} ({p.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
