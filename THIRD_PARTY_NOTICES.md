# Third-Party Open-Source Notices

This file documents all third-party open-source software components used or linked by **Universe Workstation (寰宇工作台)**, along with license elections and verbatim copyright, permission, and attribution notices required by their respective licenses.

> Full license texts are stored in [`licenses/`](licenses/) and [`crates/workspace-nautilus/LICENSE`](crates/workspace-nautilus/LICENSE).

---

## 1. Repository License Boundaries & Multi-License Elections

| Component / Artifact | Path | License | Notes |
| :--- | :--- | :--- | :--- |
| Core library, Linux & Android daemons, GTK4 UI, quick-send dialog, Android App & Bridge DEX | `crates/workspace-core`, `crates/workspace-linux`, `crates/workspace-share`, `crates/workspace-android`, `android/app`, `android/bridge-dex` | `MIT` | Governed by root [`LICENSE`](LICENSE) |
| GNOME Nautilus 4 context-menu extension (`libnautilus_workspace.so`) | `crates/workspace-nautilus` | `GPL-3.0-or-later` | Governed by [`crates/workspace-nautilus/LICENSE`](crates/workspace-nautilus/LICENSE). Launches `workspace-share` strictly via external OS process execution (`execve`), preserving a clean executable boundary (mere aggregation). |

**Multi-License Election**: Where dual- or multi-licensed dependencies are offered under disjunctive terms (such as `MIT OR Apache-2.0`, `Apache-2.0 OR ISC OR MIT`, `BSD-2-Clause OR Apache-2.0 OR MIT`, `CC0-1.0 OR Apache-2.0`, or `MIT OR Apache-2.0 OR LGPL-2.1-or-later`), Universe Workstation elects **MIT** or **Apache-2.0** (or the more permissive option) for redistribution.

---

## 2. Dynamically Linked Linux Shared Libraries (LGPL / GPL)

Linux desktop binaries dynamically link against host system shared libraries (`.so`). No copies of these libraries are embedded in this repository or statically linked into our MIT binaries:

| System Library | Observed Upstream License | Compliance Notice & Source Availability |
| :--- | :--- | :--- |
| **GTK 4** (`libgtk-4`) | `LGPL-2.1-or-later` | Pursuant to LGPL v2.1 Section 6, users may freely upgrade, modify, or relink host GTK 4 shared libraries. Full text in [`licenses/LGPL-2.1-or-later.txt`](licenses/LGPL-2.1-or-later.txt); source is available from GNOME or your Linux distribution. |
| **Libadwaita 1** (`libadwaita-1`) | `LGPL-2.1-or-later` | Dynamically linked shared library. See [`licenses/LGPL-2.1-or-later.txt`](licenses/LGPL-2.1-or-later.txt). |
| **GLib / GObject / GIO** | `LGPL-2.1-or-later` | Dynamically linked shared library. See [`licenses/LGPL-2.1-or-later.txt`](licenses/LGPL-2.1-or-later.txt). |
| **Cairo** | `LGPL-2.1-only OR MPL-1.1` | Dynamically linked shared library. See [`licenses/LGPL-2.1-or-later.txt`](licenses/LGPL-2.1-or-later.txt). |
| **Pango** & **GdkPixbuf** | `LGPL-2.0-or-later` | Dynamically linked shared libraries. |
| **libnautilus-extension-4** | `LGPL-2.1-or-later` / `GPL-2.0-or-later` / `GPL-3.0-or-later` | Linked exclusively by `crates/workspace-nautilus` (`libnautilus_workspace.so`), which is independently licensed under **`GPL-3.0-or-later`** (see [`licenses/GPL-3.0-or-later.txt`](licenses/GPL-3.0-or-later.txt)). |

---

## 3. Android, LibXposed & Build Tooling Dependencies

| Component | Coordinate / Artifact | License | Attribution |
| :--- | :--- | :--- | :--- |
| **AndroidX Jetpack** (`core-ktx`, `activity-compose`, `compose-*`, `lifecycle-*`) | `androidx.*` | `Apache-2.0` | Copyright The Android Open Source Project ([`licenses/APACHE-2.0.txt`](licenses/APACHE-2.0.txt)) |
| **Kotlin & Compose Compiler** | `org.jetbrains.kotlin` | `Apache-2.0` | Copyright JetBrains s.r.o. and Kotlin Programming Language contributors |
| **LibXposed API & Service** (`102.0.0`) | `io.github.libxposed:api`, `service`, `interface` | `Apache-2.0` | Copyright LibXposed Contributors (<https://github.com/libxposed>) |
| **Gradle Wrapper** | `android/gradle/wrapper/gradle-wrapper.jar` | `Apache-2.0` | Copyright Gradle Inc. |
| **JUnit 4** (`4.13.2`, `testImplementation` only) | `junit:junit` | `EPL-1.0` | Copyright JUnit Contributors (unit tests only; not distributed in APK) |

---

## 4. Rust Crates Requiring Verbatim Attribution / Notices

### 4.1 `ring` (`0.17.14`, `Apache-2.0 AND ISC`)

- Portions derived from BoringSSL are licensed under the Apache License 2.0 ([`licenses/APACHE-2.0.txt`](licenses/APACHE-2.0.txt)) and ISC licenses.
- Original code in `*ring*` carries the following ISC notice:

```text
Copyright 2015-2025 Brian Smith.

Permission to use, copy, modify, and/or distribute this software for any
purpose with or without fee is hereby granted, provided that the above
copyright notice and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY
SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN ACTION
OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF OR IN
CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
```

### 4.2 `rustls-webpki` (`0.103.17`, `ISC`), `untrusted` (`0.9.0`, `ISC`) & `rustls` (`0.23.46`, `Apache-2.0 OR ISC OR MIT`)

```text
rustls-webpki: Copyright 2015 Brian Smith.
untrusted:     Copyright 2015-2016 Brian Smith.
rustls:        Copyright (c) 2016, Joseph Birr-Pixton <jpixton@gmail.com>

Permission to use, copy, modify, and/or distribute this software for any
purpose with or without fee is hereby granted, provided that the above
copyright notice and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY
SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF OR
IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
```

### 4.3 `ed25519-dalek` (`2.2.0`), `curve25519-dalek` (`4.1.3`) & `subtle` (`2.6.1`) (`BSD-3-Clause`)

```text
ed25519-dalek:    Copyright (c) 2017-2019 isis agora lovecruft. All rights reserved.
curve25519-dalek: Copyright (c) 2016-2021 isis agora lovecruft. All rights reserved.
                  Copyright (c) 2016-2021 Henry de Valence. All rights reserved.
                  (Portions derived from Adam Langley's Go ed25519: Copyright (c) 2012 The Go Authors.)
subtle:           Copyright (c) 2016-2017 Isis Agora Lovecruft, Henry de Valence. All rights reserved.
                  Copyright (c) 2016-2024 Isis Agora Lovecruft. All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are
met:

1. Redistributions of source code must retain the above copyright
notice, this list of conditions and the following disclaimer.

2. Redistributions in binary form must reproduce the above copyright
notice, this list of conditions and the following disclaimer in the
documentation and/or other materials provided with the distribution.

3. Neither the name of the copyright holder nor the names of its
contributors may be used to endorse or promote products derived from
this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS
IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED
TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A
PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED
TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR
PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF
LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING
NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```

### 4.4 `matchit` (`0.7.3`, `MIT AND BSD-3-Clause`)

```text
MIT License — Copyright (c) 2022 Ibraheem Ahmed
BSD 3-Clause License (httprouter) — Copyright (c) 2013, Julien Schmidt. All rights reserved.
```

### 4.5 `option-ext` (`0.2.0`, `MPL-2.0`)

`option-ext` (transitive dependency via `dirs`) is used unmodified under the **Mozilla Public License Version 2.0 (MPL-2.0)**. Pursuant to MPL-2.0 Section 3.2:
- Full license text is provided in [`licenses/MPL-2.0.txt`](licenses/MPL-2.0.txt).
- Complete Source Code Form is available from upstream at <https://github.com/soc/option-ext> and <https://crates.io/crates/option-ext/0.2.0>.

### 4.6 `unicode-ident` (`1.0.27`, `(MIT OR Apache-2.0) AND Unicode-3.0`)

```text
UNICODE LICENSE V3 — Copyright © 1991-2023 Unicode, Inc.

Permission is hereby granted, free of charge, to any person obtaining a
copy of data files and any associated documentation (the "Data Files") or
software and any associated documentation (the "Software") to deal in the
Data Files or Software without restriction, including without limitation
the rights to use, copy, modify, merge, publish, distribute, and/or sell
copies of the Data Files or Software, and to permit persons to whom the
Data Files or Software are furnished to do so, provided that either (a)
this copyright and permission notice appear with all copies of the Data
Files or Software, or (b) this copyright and permission notice appear in
associated Documentation.

THE DATA FILES AND SOFTWARE ARE PROVIDED "AS IS", WITHOUT WARRANTY OF ANY
KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT OF
THIRD PARTY RIGHTS.
```

---

## 5. Direct Rust Workspace Dependencies (`Cargo.toml` / `Cargo.lock`)

| Crate | Version | License Expression |
| :--- | :--- | :--- |
| `anyhow` | `1.0.104` | `MIT OR Apache-2.0` |
| `axum` | `0.7.9` | `MIT` |
| `base64` | `0.22.1` | `MIT OR Apache-2.0` |
| `blake3` | `1.8.7` | `CC0-1.0 OR Apache-2.0 OR Apache-2.0 WITH LLVM-exception` |
| `bytes` | `1.12.1` | `MIT` |
| `cc` (build) | `1.11.2` | `MIT OR Apache-2.0` |
| `clap` | `4.6.7` | `MIT OR Apache-2.0` |
| `dirs` | `5.0.1` | `MIT OR Apache-2.0` |
| `ed25519-dalek` | `2.2.0` | `BSD-3-Clause` |
| `futures-util` | `0.3.34` | `MIT OR Apache-2.0` |
| `hkdf` | `0.12.4` | `MIT OR Apache-2.0` |
| `hostname` | `0.4.2` | `MIT` |
| `http` | `1.5.0` | `MIT OR Apache-2.0` |
| `http-body` | `1.1.0` | `MIT` |
| `http-body-util` | `0.1.5` | `MIT` |
| `hyper` | `1.12.0` | `MIT` |
| `hyper-util` | `0.1.21` | `MIT` |
| `libc` | `0.2.191` | `MIT OR Apache-2.0` |
| `pkg-config` (build) | `0.3.34` | `MIT OR Apache-2.0` |
| `rand` | `0.8.5` | `MIT OR Apache-2.0` |
| `rcgen` | `0.14.10` | `MIT OR Apache-2.0` |
| `rustls` | `0.23.46` | `Apache-2.0 OR ISC OR MIT` |
| `rustls-pki-types` | `1.16.0` | `MIT OR Apache-2.0` |
| `serde` | `1.0.229` | `MIT OR Apache-2.0` |
| `serde_json` | `1.0.154` | `MIT OR Apache-2.0` |
| `sha2` | `0.10.9` | `MIT OR Apache-2.0` |
| `socket2` | `0.6.5` | `MIT OR Apache-2.0` |
| `tempfile` (dev) | `3.27.0` | `MIT OR Apache-2.0` |
| `thiserror` | `2.0.21` | `MIT OR Apache-2.0` |
| `tokio` | `1.54.1` | `MIT` |
| `tokio-rustls` | `0.26.6` | `MIT OR Apache-2.0` |
| `tokio-util` | `0.7.20` | `MIT` |
| `toml` | `0.8.23` | `MIT OR Apache-2.0` |
| `tower` | `0.5.3` | `MIT` |
| `tower-service` | `0.3.3` | `MIT` |
| `tracing` | `0.1.44` | `MIT` |
| `tracing-subscriber` | `0.3.23` | `MIT` |
| `uuid` | `1.29.2` | `Apache-2.0 OR MIT` |
| `x509-parser` | `0.18.1` | `MIT OR Apache-2.0` |
| `zbus` | `5.19.0` | `MIT` |
| `zeroize` | `1.9.1` | `Apache-2.0 OR MIT` |

---

## 6. Trademarks & Interoperability Disclaimer

- **Linux**, **GNOME**, **Nautilus**, **Android**, **OPPO**, **OnePlus**, **ColorOS**, **OShare**, **KernelSU**, and **LSPosed** are trademarks or registered trademarks of their respective owners.
- **Universe Workstation** is an independent open-source project and is not affiliated with, endorsed by, or sponsored by any of the above organizations. System UI and OShare interoperability hooks operate exclusively via standard runtime LibXposed API reflection; no proprietary vendor binaries or SDKs are redistributed in this repository.
