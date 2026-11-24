// SPDX-License-Identifier: MIT
// Self-contained AppImage Type 2 ELF runtime with built-in gzip SquashFS 4.0 extractor.
// Requires only libc and zlib (no libfuse.so.2 runtime dependency).

#define _GNU_SOURCE
#include <elf.h>
#include <errno.h>
#include <fcntl.h>
#include <libgen.h>
#include <limits.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/file.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>
#include <zlib.h>

#define SQFS_MAGIC 0x73717368u
#define SQFS_META_BLOCK_SIZE 8192u
#define SQFS_COMPRESSED_BIT (1u << 24)

#define SQFS_DIR_TYPE 1u
#define SQFS_REG_TYPE 2u
#define SQFS_SYMLINK_TYPE 3u
#define SQFS_LDIR_TYPE 8u
#define SQFS_LREG_TYPE 9u
#define SQFS_LSYMLINK_TYPE 10u

typedef struct __attribute__((packed)) {
    uint32_t s_magic;
    uint32_t inodes;
    int32_t mkfs_time;
    uint32_t block_size;
    uint32_t fragments;
    uint16_t compression;
    uint16_t block_log;
    uint16_t flags;
    uint16_t no_ids;
    uint16_t s_major;
    uint16_t s_minor;
    uint64_t root_inode;
    uint64_t bytes_used;
    uint64_t id_table_start;
    uint64_t xattr_id_table_start;
    uint64_t inode_table_start;
    uint64_t directory_table_start;
    uint64_t fragment_table_start;
    uint64_t export_table_start;
} SqfsSuperblock;

_Static_assert(sizeof(SqfsSuperblock) == 96, "SqfsSuperblock must be 96 bytes");

typedef struct __attribute__((packed)) {
    uint16_t inode_type;
    uint16_t mode;
    uint16_t uid;
    uint16_t guid;
    int32_t mtime;
    uint32_t inode_number;
} SqfsBaseInode;

typedef struct __attribute__((packed)) {
    uint32_t start_block;
    uint32_t nlink;
    uint16_t file_size;
    uint16_t offset;
    uint32_t parent_inode;
} SqfsDirInodeBody;

typedef struct __attribute__((packed)) {
    uint32_t nlink;
    uint32_t file_size;
    uint32_t start_block;
    uint32_t parent_inode;
    uint16_t i_count;
    uint16_t offset;
    uint32_t xattr;
} SqfsLdirInodeBody;

typedef struct __attribute__((packed)) {
    uint32_t start_block;
    uint32_t fragment;
    uint32_t offset;
    uint32_t file_size;
} SqfsRegInodeBody;

typedef struct __attribute__((packed)) {
    uint64_t start_block;
    uint64_t file_size;
    uint64_t sparse;
    uint32_t nlink;
    uint32_t fragment;
    uint32_t offset;
    uint32_t xattr;
} SqfsLregInodeBody;

typedef struct __attribute__((packed)) {
    uint32_t nlink;
    uint32_t symlink_size;
} SqfsSymlinkBody;

typedef struct __attribute__((packed)) {
    uint32_t count;
    uint32_t start_block;
    int32_t inode_number;
} SqfsDirHeader;

typedef struct __attribute__((packed)) {
    uint16_t offset;
    int16_t inode_diff;
    uint16_t type;
    uint16_t size;
} SqfsDirEntry;

typedef struct {
    int fd;
    uint64_t sqfs_off;
    SqfsSuperblock sb;
} SqfsReader;

typedef struct {
    uint64_t block_disk_rel;
    uint32_t off;
    uint8_t buf[SQFS_META_BLOCK_SIZE];
    uint32_t buf_len;
    uint64_t next_block_disk_rel;
    int loaded;
} MetaCursor;

static int pread_exact(int fd, void *buf, size_t n, uint64_t off) {
    uint8_t *p = (uint8_t *)buf;
    size_t done = 0;
    while (done < n) {
        ssize_t r = pread(fd, p + done, n - done, (off_t)(off + done));
        if (r < 0) {
            if (errno == EINTR) continue;
            return -1;
        }
        if (r == 0) {
            errno = EIO;
            return -1;
        }
        done += (size_t)r;
    }
    return 0;
}

static int write_exact(int fd, const void *buf, size_t n) {
    const uint8_t *p = (const uint8_t *)buf;
    size_t done = 0;
    while (done < n) {
        ssize_t w = write(fd, p + done, n - done);
        if (w < 0) {
            if (errno == EINTR) continue;
            return -1;
        }
        done += (size_t)w;
    }
    return 0;
}

static int meta_load_block(const SqfsReader *r, MetaCursor *cur) {
    uint16_t hdr = 0;
    uint64_t abs_off = r->sqfs_off + cur->block_disk_rel;
    if (pread_exact(r->fd, &hdr, sizeof(hdr), abs_off) != 0) {
        return -1;
    }
    uint32_t disk_len = (uint32_t)(hdr & 0x7fffu);
    int uncomp = (hdr & 0x8000u) != 0;
    if (disk_len == 0 || disk_len > SQFS_META_BLOCK_SIZE) {
        errno = EINVAL;
        return -1;
    }
    uint8_t raw[SQFS_META_BLOCK_SIZE];
    if (pread_exact(r->fd, raw, disk_len, abs_off + 2u) != 0) {
        return -1;
    }
    if (uncomp) {
        memcpy(cur->buf, raw, disk_len);
        cur->buf_len = disk_len;
    } else {
        uLongf dst_len = sizeof(cur->buf);
        if (uncompress(cur->buf, &dst_len, raw, disk_len) != Z_OK || dst_len > SQFS_META_BLOCK_SIZE) {
            errno = EIO;
            return -1;
        }
        cur->buf_len = (uint32_t)dst_len;
    }
    cur->next_block_disk_rel = cur->block_disk_rel + 2u + disk_len;
    cur->loaded = 1;
    return 0;
}

static void meta_seek(MetaCursor *cur, uint64_t table_start, uint64_t block_rel, uint32_t off) {
    memset(cur, 0, sizeof(*cur));
    cur->block_disk_rel = table_start + block_rel;
    cur->off = off;
}

static int meta_read(const SqfsReader *r, MetaCursor *cur, void *dst, size_t n) {
    uint8_t *out = (uint8_t *)dst;
    size_t rem = n;
    while (rem > 0) {
        if (!cur->loaded) {
            if (meta_load_block(r, cur) != 0) return -1;
            if (cur->off > cur->buf_len) {
                errno = EINVAL;
                return -1;
            }
        }
        uint32_t avail = cur->buf_len - cur->off;
        if (avail == 0) {
            cur->block_disk_rel = cur->next_block_disk_rel;
            cur->off = 0;
            cur->loaded = 0;
            continue;
        }
        size_t take = rem < avail ? rem : avail;
        memcpy(out, cur->buf + cur->off, take);
        out += take;
        cur->off += (uint32_t)take;
        rem -= take;
    }
    return 0;
}

static int find_sqfs_offset(int fd, uint64_t *out_off) {
    struct stat st;
    if (fstat(fd, &st) != 0 || st.st_size < 192) {
        return -1;
    }
    uint64_t file_size = (uint64_t)st.st_size;

    Elf64_Ehdr ehdr;
    if (pread_exact(fd, &ehdr, sizeof(ehdr), 0) != 0) {
        return -1;
    }
    if (memcmp(ehdr.e_ident, ELFMAG, SELFMAG) != 0) {
        errno = EINVAL;
        return -1;
    }

    uint64_t elf_end = 4096;
    if (ehdr.e_shoff > 0 && ehdr.e_shentsize > 0 && ehdr.e_shnum > 0) {
        elf_end = ehdr.e_shoff + (uint64_t)ehdr.e_shentsize * (uint64_t)ehdr.e_shnum;
    }
    uint64_t start = elf_end & ~4095ULL;
    if (start < 4096) start = 4096;

    for (uint64_t off = start; off + sizeof(SqfsSuperblock) <= file_size; off += 4096) {
        SqfsSuperblock sb;
        if (pread_exact(fd, &sb, sizeof(sb), off) != 0) {
            return -1;
        }
        if (sb.s_magic == SQFS_MAGIC && sb.s_major == 4 && sb.s_minor == 0 &&
            sb.bytes_used > sizeof(SqfsSuperblock) && off + sb.bytes_used <= file_size) {
            *out_off = off;
            return 0;
        }
    }
    errno = ENOENT;
    return -1;
}

static int extract_reg_file(
    const SqfsReader *r,
    MetaCursor *cur,
    const char *path,
    uint16_t mode,
    uint64_t start_block,
    uint64_t file_size
) {
    uint64_t block_size = r->sb.block_size;
    if (block_size == 0 || block_size > (4u << 20)) {
        errno = EINVAL;
        return -1;
    }
    uint64_t num_blocks = file_size == 0 ? 0 : (file_size + block_size - 1) / block_size;
    uint32_t *block_sizes = NULL;
    if (num_blocks > 0) {
        block_sizes = (uint32_t *)malloc((size_t)num_blocks * sizeof(uint32_t));
        if (!block_sizes) return -1;
        if (meta_read(r, cur, block_sizes, (size_t)num_blocks * sizeof(uint32_t)) != 0) {
            free(block_sizes);
            return -1;
        }
    }

    int out_fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, (mode_t)(mode & 0777u));
    if (out_fd < 0) {
        free(block_sizes);
        return -1;
    }

    uint8_t *raw_buf = (uint8_t *)malloc((size_t)block_size);
    uint8_t *out_buf = (uint8_t *)malloc((size_t)block_size);
    if (!raw_buf || !out_buf) {
        free(raw_buf);
        free(out_buf);
        free(block_sizes);
        close(out_fd);
        return -1;
    }

    uint64_t data_off = r->sqfs_off + start_block;
    uint64_t rem = file_size;
    int rc = 0;

    for (uint64_t i = 0; i < num_blocks; ++i) {
        uint32_t bsz = block_sizes[i];
        uint32_t disk_len = bsz & 0x00ffffffu;
        int uncomp = (bsz & SQFS_COMPRESSED_BIT) != 0;
        uint64_t expect = rem < block_size ? rem : block_size;

        if (disk_len == 0) {
            memset(out_buf, 0, (size_t)expect);
            if (write_exact(out_fd, out_buf, (size_t)expect) != 0) {
                rc = -1;
                break;
            }
        } else {
            if (disk_len > block_size) {
                errno = EINVAL;
                rc = -1;
                break;
            }
            if (pread_exact(r->fd, raw_buf, disk_len, data_off) != 0) {
                rc = -1;
                break;
            }
            data_off += disk_len;
            if (uncomp) {
                size_t to_write = disk_len < expect ? disk_len : (size_t)expect;
                if (write_exact(out_fd, raw_buf, to_write) != 0) {
                    rc = -1;
                    break;
                }
            } else {
                uLongf dst_len = (uLongf)block_size;
                if (uncompress(out_buf, &dst_len, raw_buf, disk_len) != Z_OK || dst_len != expect) {
                    errno = EIO;
                    rc = -1;
                    break;
                }
                if (write_exact(out_fd, out_buf, (size_t)dst_len) != 0) {
                    rc = -1;
                    break;
                }
            }
        }
        rem -= expect;
    }

    if (rc == 0 && fchmod(out_fd, (mode_t)(mode & 0777u)) != 0) {
        rc = -1;
    }

    free(raw_buf);
    free(out_buf);
    free(block_sizes);
    if (close(out_fd) != 0) rc = -1;
    return rc;
}

static int extract_inode(const SqfsReader *r, uint64_t inode_ref, const char *dst_path, int depth) {
    if (depth > 64) {
        errno = ELOOP;
        return -1;
    }

    uint64_t block_rel = inode_ref >> 16;
    uint32_t block_off = (uint32_t)(inode_ref & 0xffffu);

    MetaCursor cur;
    meta_seek(&cur, r->sb.inode_table_start, block_rel, block_off);

    SqfsBaseInode base;
    if (meta_read(r, &cur, &base, sizeof(base)) != 0) {
        return -1;
    }

    if (base.inode_type == SQFS_DIR_TYPE || base.inode_type == SQFS_LDIR_TYPE) {
        uint64_t dir_start = 0;
        uint32_t dir_off = 0;
        uint32_t dir_size = 0;
        if (base.inode_type == SQFS_DIR_TYPE) {
            SqfsDirInodeBody body;
            if (meta_read(r, &cur, &body, sizeof(body)) != 0) return -1;
            dir_start = body.start_block;
            dir_off = body.offset;
            dir_size = body.file_size >= 3 ? (uint32_t)(body.file_size - 3) : 0;
        } else {
            SqfsLdirInodeBody body;
            if (meta_read(r, &cur, &body, sizeof(body)) != 0) return -1;
            dir_start = body.start_block;
            dir_off = body.offset;
            dir_size = body.file_size >= 3 ? (uint32_t)(body.file_size - 3) : 0;
        }

        if (mkdir(dst_path, 0755) != 0 && errno != EEXIST) {
            return -1;
        }

        MetaCursor dcur;
        meta_seek(&dcur, r->sb.directory_table_start, dir_start, dir_off);
        uint32_t read_bytes = 0;
        while (read_bytes < dir_size) {
            SqfsDirHeader dh;
            if (meta_read(r, &dcur, &dh, sizeof(dh)) != 0) return -1;
            read_bytes += (uint32_t)sizeof(dh);
            uint32_t n_entries = dh.count + 1u;
            for (uint32_t i = 0; i < n_entries; ++i) {
                SqfsDirEntry ent;
                if (meta_read(r, &dcur, &ent, sizeof(ent)) != 0) return -1;
                read_bytes += (uint32_t)sizeof(ent);
                size_t name_len = (size_t)ent.size + 1u;
                if (name_len == 0 || name_len > 256) {
                    errno = ENAMETOOLONG;
                    return -1;
                }
                char name[257];
                if (meta_read(r, &dcur, name, name_len) != 0) return -1;
                name[name_len] = '\0';
                read_bytes += (uint32_t)name_len;

                if (strcmp(name, ".") == 0 || strcmp(name, "..") == 0 || strchr(name, '/') != NULL) {
                    errno = EINVAL;
                    return -1;
                }

                char child_path[PATH_MAX];
                if (snprintf(child_path, sizeof(child_path), "%s/%s", dst_path, name) >= (int)sizeof(child_path)) {
                    errno = ENAMETOOLONG;
                    return -1;
                }
                uint64_t child_ref = ((uint64_t)dh.start_block << 16) | (uint64_t)ent.offset;
                if (extract_inode(r, child_ref, child_path, depth + 1) != 0) {
                    return -1;
                }
            }
        }
        chmod(dst_path, (mode_t)(base.mode & 0777u));
        return 0;
    }

    if (base.inode_type == SQFS_REG_TYPE) {
        SqfsRegInodeBody body;
        if (meta_read(r, &cur, &body, sizeof(body)) != 0) return -1;
        return extract_reg_file(r, &cur, dst_path, base.mode, body.start_block, body.file_size);
    }

    if (base.inode_type == SQFS_LREG_TYPE) {
        SqfsLregInodeBody body;
        if (meta_read(r, &cur, &body, sizeof(body)) != 0) return -1;
        return extract_reg_file(r, &cur, dst_path, base.mode, body.start_block, body.file_size);
    }

    if (base.inode_type == SQFS_SYMLINK_TYPE || base.inode_type == SQFS_LSYMLINK_TYPE) {
        SqfsSymlinkBody body;
        if (meta_read(r, &cur, &body, sizeof(body)) != 0) return -1;
        if (body.symlink_size == 0 || body.symlink_size >= PATH_MAX) {
            errno = ENAMETOOLONG;
            return -1;
        }
        char target[PATH_MAX];
        if (meta_read(r, &cur, target, body.symlink_size) != 0) return -1;
        target[body.symlink_size] = '\0';
        unlink(dst_path);
        return symlink(target, dst_path);
    }

    errno = ENOTSUP;
    return -1;
}

static int extract_appdir(int fd, uint64_t sqfs_off, const char *dst_dir) {
    SqfsReader r;
    memset(&r, 0, sizeof(r));
    r.fd = fd;
    r.sqfs_off = sqfs_off;
    if (pread_exact(fd, &r.sb, sizeof(r.sb), sqfs_off) != 0) {
        return -1;
    }
    if (r.sb.s_magic != SQFS_MAGIC || r.sb.s_major != 4 || r.sb.s_minor != 0 || r.sb.compression != 1) {
        errno = EINVAL;
        return -1;
    }
    return extract_inode(&r, r.sb.root_inode, dst_dir, 0);
}

static uint64_t fnv1a_self_id(int fd, uint64_t sqfs_off) {
    struct stat st;
    if (fstat(fd, &st) != 0) return sqfs_off;
    uint64_t h = 14695981039346656037ULL;
    uint64_t vals[3] = { (uint64_t)st.st_size, (uint64_t)st.st_mtime, sqfs_off };
    const uint8_t *p = (const uint8_t *)vals;
    for (size_t i = 0; i < sizeof(vals); ++i) {
        h ^= p[i];
        h *= 1099511628211ULL;
    }
    return h;
}

static void print_help(const char *argv0) {
    fprintf(
        stderr,
        "AppImage runtime options for %s:\n"
        "  --appimage-extract           Extract embedded AppDir to ./squashfs-root\n"
        "  --appimage-extract-and-run   Extract to runtime cache and execute AppRun\n"
        "  --appimage-offset            Print byte offset of embedded SquashFS payload\n"
        "  --appimage-version           Print AppImage runtime version\n"
        "  --appimage-help              Show this help\n",
        argv0
    );
}

int main(int argc, char **argv) {
    char self_path[PATH_MAX];
    ssize_t self_len = readlink("/proc/self/exe", self_path, sizeof(self_path) - 1);
    if (self_len < 0) {
        perror("readlink(/proc/self/exe)");
        return 127;
    }
    self_path[self_len] = '\0';

    int fd = open(self_path, O_RDONLY);
    if (fd < 0) {
        perror("open(/proc/self/exe)");
        return 127;
    }

    uint64_t sqfs_off = 0;
    if (find_sqfs_offset(fd, &sqfs_off) != 0) {
        fprintf(stderr, "Failed to locate SquashFS payload in AppImage: %s\n", strerror(errno));
        close(fd);
        return 127;
    }

    int force_extract_and_run = 0;
    if (argc >= 2) {
        if (strcmp(argv[1], "--appimage-offset") == 0) {
            printf("%llu\n", (unsigned long long)sqfs_off);
            close(fd);
            return 0;
        }
        if (strcmp(argv[1], "--appimage-version") == 0) {
            printf("AppImage Type 2 (Universe Workstation)\n");
            close(fd);
            return 0;
        }
        if (strcmp(argv[1], "--appimage-help") == 0) {
            print_help(argv[0]);
            close(fd);
            return 0;
        }
        if (strcmp(argv[1], "--appimage-extract") == 0) {
            if (extract_appdir(fd, sqfs_off, "squashfs-root") != 0) {
                fprintf(stderr, "AppImage extraction failed: %s\n", strerror(errno));
                close(fd);
                return 1;
            }
            close(fd);
            return 0;
        }
        if (strcmp(argv[1], "--appimage-extract-and-run") == 0) {
            force_extract_and_run = 1;
        }
    }

    uint64_t sig = fnv1a_self_id(fd, sqfs_off);
    const char *rt = getenv("XDG_RUNTIME_DIR");
    char appdir[PATH_MAX];
    char lockpath[PATH_MAX + 32];
    char readypath[PATH_MAX + 32];

    if (rt && rt[0] == '/') {
        snprintf(appdir, sizeof(appdir), "%s/universe-workstation-appimage-%016llx", rt, (unsigned long long)sig);
    } else {
        snprintf(appdir, sizeof(appdir), "/tmp/universe-workstation-appimage-%u-%016llx", (unsigned)getuid(), (unsigned long long)sig);
    }
    snprintf(lockpath, sizeof(lockpath), "%s.lock", appdir);
    snprintf(readypath, sizeof(readypath), "%s/.appimage_ready", appdir);

    int lock_fd = open(lockpath, O_CREAT | O_RDWR, 0600);
    if (lock_fd >= 0) {
        (void)flock(lock_fd, LOCK_EX);
    }

    if (access(readypath, F_OK) != 0) {
        if (extract_appdir(fd, sqfs_off, appdir) != 0) {
            fprintf(stderr, "AppImage extract-and-run failed (%s): %s\n", appdir, strerror(errno));
            if (lock_fd >= 0) {
                flock(lock_fd, LOCK_UN);
                close(lock_fd);
            }
            close(fd);
            return 127;
        }
        int rfd = open(readypath, O_CREAT | O_WRONLY | O_TRUNC, 0644);
        if (rfd >= 0) close(rfd);
    }

    if (lock_fd >= 0) {
        flock(lock_fd, LOCK_UN);
        close(lock_fd);
    }
    close(fd);

    char cwd[PATH_MAX];
    if (getcwd(cwd, sizeof(cwd)) != NULL) {
        setenv("OWD", cwd, 1);
    }
    setenv("APPIMAGE", self_path, 1);
    setenv("APPDIR", appdir, 1);
    setenv("ARGV0", argv[0], 1);

    char apprun[PATH_MAX + 32];
    snprintf(apprun, sizeof(apprun), "%s/AppRun", appdir);

    int shift = force_extract_and_run ? 1 : 0;
    int child_argc = argc - shift;
    char **child_argv = (char **)calloc((size_t)child_argc + 1u, sizeof(char *));
    if (!child_argv) {
        perror("calloc");
        return 127;
    }
    child_argv[0] = apprun;
    for (int i = 1; i < child_argc; ++i) {
        child_argv[i] = argv[i + shift];
    }
    child_argv[child_argc] = NULL;

    execv(apprun, child_argv);
    fprintf(stderr, "execv(%s) failed: %s\n", apprun, strerror(errno));
    free(child_argv);
    return 127;
}
