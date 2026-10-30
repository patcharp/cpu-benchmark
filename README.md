# cpubench

โปรแกรม benchmark CPU สำหรับเปรียบเทียบเซิร์ฟเวอร์ **Intel x86_64** กับ **ARM (aarch64)** — รันคำสั่งเดียวได้รายงานทันที
(JSON + Markdown + HTML) แล้วนำรายงานจากสองเครื่องมา `compare` กัน

เขียนด้วย **Rust** เพราะ LLVM สร้างโค้ดคุณภาพเท่ากันบนทั้งสองสถาปัตยกรรม (ถ้าคอมไพเลอร์ทำโค้ดฝั่งหนึ่งแย่กว่า
เรากำลังวัดคอมไพเลอร์ ไม่ใช่ CPU) ไม่มี GC ที่ทำให้ผลแกว่ง และแจกเป็น static binary ไฟล์เดียวได้

## เริ่มใช้งาน

```bash
make dist                      # ได้ dist/cpubench-x86_64, -x86_64-v3, -aarch64 (static ไม่ต้องลงอะไรบนเซิร์ฟเวอร์)

# บนเซิร์ฟเวอร์แต่ละตัว (copy ไฟล์ไปแล้วรันได้เลย)
./cpubench-aarch64 run --label kunpeng      # profile full เป็นค่า default
./cpubench-x86_64  run --label xeon

# เปรียบเทียบ (A = เครื่องเดิม, B = เครื่องใหม่)
./cpubench-x86_64 compare xeon.json kunpeng.json \
    --cost-a 500000 --cost-b 450000 --watts-a 700 --watts-b 560
```

ผลลัพธ์อยู่ใน `./cpubench-results/` (`<label>.json` คือข้อมูลดิบ, `.md` และ `.html` ไว้อ่านและแชร์)
คำสั่งอื่น: `info` (ดูสเปกที่ตรวจพบ), `list` (รายการ kernel), `report file.json` (สร้าง md/html ใหม่จาก JSON)

### ชื่อสถาปัตยกรรม: `aarch64` กับ `arm64` คือสิ่งเดียวกัน

ทั้งสองคือ ARM 64 บิต (ARMv8-A ขึ้นไป) ต่างกันแค่ชื่อที่แต่ละวงการเรียก ไม่ใช่คนละชุดคำสั่ง:

| ชื่อ | ใช้ที่ไหน |
|---|---|
| `aarch64` | ชื่อทางการของ ARM, Linux kernel (`uname -m`), GNU/LLVM toolchain, Rust (`aarch64-unknown-linux-musl`) |
| `arm64` | Apple, Go (`GOARCH=arm64`), Docker (`--platform linux/arm64`), Debian/Ubuntu (`dpkg --print-architecture`), Windows |

โปรเจกต์นี้ใช้ชื่อ `aarch64` (ไฟล์ `cpubench-aarch64`, ค่า `arch` ในรายงาน) ถ้าเครื่องปลายทางรายงานว่า `arm64` ให้ใช้ไฟล์เดียวกัน
สิ่งที่ใช้ไม่ได้คือ ARM **32 บิต** (`armv7l`, `armhf`) ซึ่งเป็นคนละสถาปัตยกรรม; Kunpeng 920 และ Raspberry Pi 5 เป็น 64 บิต ใช้ได้

### ตัวเลือกสำคัญของ `run`

| ตัวเลือก | ความหมาย |
|---|---|
| `--profile full` (default) | warm-up 1 วิ + 7 samples × 2 วิ ต่อการวัด (≈ 10–11 นาทีทั้งชุด) เป็น profile เดียวที่ใช้ตัดสินใจ |
| `--profile quick` / `standard` | เร็วกว่า ใช้ทดสอบว่ารันได้เท่านั้น รายงานจะติดป้ายเตือน |
| `--cpus 0-63` | จำกัดและ pin thread ไว้ที่ CPU ชุดนี้ (เช่น socket เดียว) ใช้เทียบ socket ต่อ socket |
| `--threads N`, `--pin` | จำนวน thread ของรอบ multi-thread / ตรึง thread กับ CPU |
| `--only a,b` / `--skip a,b` | เลือกหรือข้าม kernel (kernel ที่ข้ามจะถูกบันทึกเป็น `skipped` พร้อมเหตุผล ไม่หายไปเงียบๆ) |
| `--mode single\|multi\|both` | วัดเฉพาะ 1 thread, เฉพาะทุก thread หรือทั้งคู่ |
| `--mem-budget-mb N` | ขนาดหน่วยความจำรวมของ STREAM (default = max(2 GiB, 4× LLC รวม) และไม่เกิน 25% ของ RAM) |

## เว็บ UI: `cpubench serve` (กดรัน ดูความคืบหน้า เปิดรายงาน เทียบรายงาน ผ่านเบราว์เซอร์)

```bash
./cpubench-aarch64 serve                       # http://127.0.0.1:8080  (เฉพาะเครื่องนี้)
./cpubench-aarch64 serve --out /data/results   # เลือกโฟลเดอร์เก็บรายงาน (default ./cpubench-results)
```

หน้าเว็บมี: ข้อมูลเครื่อง, ฟอร์มเริ่มรัน (default profile = full), แถบความคืบหน้า + ผลสดทีละ kernel + log + ปุ่มยกเลิก,
ตารางรายงานทั้งหมด (เปิด html/md/json), และตัวเลือกเทียบสองรายงานพร้อมราคา/วัตต์ ไม่ใช้ CDN จึงใช้ได้บนเซิร์ฟเวอร์ที่ออฟไลน์

**เข้าจากเครื่องอื่น** (เลือกอย่างใดอย่างหนึ่ง):

```bash
ssh -L 8080:127.0.0.1:8080 user@server         # แนะนำ: ใช้ ssh tunnel แล้วเปิด http://127.0.0.1:8080 บนเครื่องคุณ
./cpubench-aarch64 serve --bind 0.0.0.0:8080   # เปิดให้เครือข่ายเข้าได้: สร้าง token อัตโนมัติ แล้วพิมพ์ URL ที่มี ?token=... ให้
./cpubench-aarch64 serve --bind 0.0.0.0:8080 --token <อย่างน้อย 8 ตัว>
```

การป้องกัน: bind ที่ไม่ใช่ loopback **บังคับ token** (เว้นแต่ใส่ `--no-auth` ซึ่งจะพิมพ์คำเตือน), รันได้ทีละงานเดียว,
ค่าจากฟอร์มทุกตัวถูกตรวจและส่งให้ตัวรันเป็นรายการ argument (ไม่ผ่าน shell) โดยโฟลเดอร์ผลลัพธ์กำหนดโดยผู้เปิดเซิร์ฟเวอร์เท่านั้น,
เปิดไฟล์รายงานได้เฉพาะชื่อที่ผ่าน whitelist (กัน path traversal), กัน DNS rebinding และ cross-site POST
ไม่มี TLS ในตัว: ถ้าต้องเปิดผ่านเครือข่ายที่ไม่ไว้ใจ ให้ใช้ ssh tunnel หรือ reverse proxy ที่มี HTTPS

การทำงาน: เซิร์ฟเวอร์สั่ง `cpubench run` เป็น process ลูก ถ้า kernel ใด crash หน้าเว็บยังอยู่และแสดงสถานะ `failed` พร้อม output สุดท้ายของตัวรัน
ปุ่มยกเลิกฆ่า process ลูก (ผลที่วัดไปแล้วถูกทิ้ง) และถ้าเซิร์ฟเวอร์ถูกปิด process ลูกจะถูกฆ่าตามเสมอ
หน้าเว็บจะถามสถานะทุก ~2.5 วินาที (เบามาก) และจะแจ้งถ้าพบการรันที่ไม่จบค้างอยู่ในโฟลเดอร์ผลลัพธ์

## Build จาก source บน OS เป้าหมาย (เลือก glibc หรือ musl เองได้)

ซอร์สอยู่ที่ https://github.com/patcharp/cpu-benchmark (public, clone ได้โดยไม่ต้อง login) เครื่องปลายทางต้องมี **git** และ **Rust toolchain**
(ติดตั้งด้วย rustup; dependency ตัวหนึ่งประกาศ rust-version 1.89 จึงแนะนำ Rust ≥ 1.89) และต้องเข้า github.com กับ crates.io ได้
(cargo ดาวน์โหลด dependency ตอน build ครั้งแรก)

```bash
git clone https://github.com/patcharp/cpu-benchmark.git && cd cpu-benchmark

# glibc (ใช้ libc ของ OS นั้น = สิ่งที่บริการจริงใช้)  -> ./target/release/cpubench
cargo build --release

# musl static (ต้อง rustup target add <arch>-unknown-linux-musl ก่อน) -> ./target/<arch>-unknown-linux-musl/release/cpubench
rustup target add aarch64-unknown-linux-musl                           # หรือ x86_64-unknown-linux-musl
cargo build --release --target aarch64-unknown-linux-musl

# ใช้ชุดคำสั่งเต็มของ CPU นั้น (AVX-512 / ฯลฯ); รายงานจะบันทึกไว้ว่าเป็น native build
RUSTFLAGS="-C target-cpu=native" cargo build --release
```

- **ใช้ commit เดียวกันทุกเครื่อง** ที่จะเอารายงานมาเทียบกัน (`git rev-parse --short HEAD` ดูได้): `compare` เตือนเมื่อเลขเวอร์ชันของ cpubench ต่างกัน
  แต่เลขเวอร์ชันไม่เปลี่ยนระหว่าง commit และรายงานยังไม่บันทึกว่า build จาก commit ไหน จึงต้องคุมเอง (`git checkout <commit>` ก่อน build)
- รายงานบันทึก libc ของ build (`glibc` / `musl`) และ `compare` จะเตือนถ้าสองรายงานใช้ libc ไม่ตรงกัน
- ถ้า `rust-lld` ไม่มีบนเครื่องปลายทาง ให้ลบบรรทัด `linker = "rust-lld"` ใน `.cargo/config.toml` สำหรับ target musl ที่ไม่ได้ใช้
- เครื่องที่ออก internet ไม่ได้: ใช้ binary สำเร็จรูปจาก `make dist` หรือ `make src-dist` (tarball ซอร์สพร้อม dependency สำหรับ `cargo build --offline`) บนเครื่องที่ออกเน็ตได้ แล้วคัดลอกไป

## สิ่งที่วัด (24 kernel, ทุกตัววัดที่ 1 thread และทุก thread)

| กลุ่ม | Kernel | ดูอะไร |
|---|---|---|
| Integer | `int.sort`, `int.sieve`, `int.mix`, `int.collatz` | sort, sieve ที่แตะ cache, throughput คูณ 64 บิต, dependent chain + branch |
| Floating point | `fp.vec_f32`, `fp.matmul_f64`, `fp.nbody` | ความกว้าง SIMD, matmul แบบ blocked, sqrt/div latency |
| Memory | `mem.copy`, `mem.triad`, `mem.memcpy_small`, `mem.lat.{32k,256k,2m,16m,128m,512m}` | bandwidth แบบ STREAM, memcpy ก้อนเล็ก (วัดตัว libc) และ latency ไล่ตั้งแต่ L1 จนถึง DRAM (single-thread) |
| Crypto | `crypto.sha256`, `crypto.sha512`, `crypto.aes256` | ใช้ hardware instruction ถ้ามี (SHA-NI, AES-NI, ARMv8 crypto) |
| Compression | `comp.deflate`, `comp.inflate` | DEFLATE level 6 บน corpus ข้อความ |
| Server workloads | `data.hashmap`, `data.json`, `data.alloc` | HashMap, serde_json, malloc/free (alloc รัน 1 thread เท่านั้น) |

ทุก kernel ใช้ข้อมูล deterministic และคืน **checksum** ซึ่งต้องตรงกันทุกเครื่องทุกสถาปัตยกรรม ถ้าไม่ตรง `compare` จะแจ้ง `CHECKSUM MISMATCH`

## หลักการ "ไม่ซ่อนอะไร"

รายงานสะท้อนสิ่งที่เกิดขึ้นจริง และมีหัวข้อ **Findings** ที่ด้านบนเสมอ:

- kernel ทุกตัวมี `status` (`ok` / `skipped` / `unsupported` / `failed`) พร้อมเหตุผล ไม่มี kernel หายจากรายงานแบบเงียบๆ
- ระบุ **backend ที่ใช้จริง** เช่น `hardware (SHA-NI)` หรือ `software fallback (CPU has no ARMv8 SHA512 extension)`
  ถ้า CPU ไม่รองรับ instruction ตัวไหน ตัวเลขที่ออกมาคือ software จริงๆ และมีป้ายบอก
- เก็บ sample ดิบ, median, min, max, stddev, CV% ทุกการวัด และเตือนเมื่อ CV > 5%
- เตือน: VM/container, cgroup limit, governor ไม่ใช่ `performance`, page size > 4 KiB, หลาย NUMA node แต่ไม่ได้ pin,
  build debug, profile ที่ไม่ใช่ full, ฟีเจอร์ ISA ที่ขาด (ไม่มี SVE, ไม่มี SHA-NI ฯลฯ)
- `compare` เตือนเมื่อสอง build เทียบกันไม่ได้ (libc ต่างกัน, flags ต่างกัน, เวอร์ชันต่างกัน) และเมื่อ checksum ไม่ตรง

## วิธีให้ผลน่าเชื่อถือ

1. **ปิดงานอื่นบนเครื่อง** และตั้ง BIOS power profile / governor เป็น `performance` (ดู `Governor` ในรายงาน)
2. รัน **อย่างน้อย 3 ครั้ง** ต่อเครื่อง แล้วดูว่า CV% ต่ำและผลใกล้กัน
3. เครื่อง 2 socket / หลาย NUMA: รันทั้งเครื่องหนึ่งรอบ และรัน `--cpus <cpu ของ socket เดียว>` อีกรอบเพื่อเทียบ socket ต่อ socket
4. ใช้ OS/kernel/THP ใกล้เคียงกัน (หรืออย่างน้อยดู `Page size / THP` ในรายงาน — ARM บางดิสโทรใช้ page 64 KiB ซึ่งเปลี่ยนผล latency ช่วง DRAM)
5. ใช้ binary ที่ build ด้วย libc เดียวกันทั้งสองฝั่ง (ชุด `make dist` เป็น musl ทั้งคู่)

## ข้อควรรู้ / ข้อจำกัด

- **ไม่ใช่ SPEC CPU** ผลคือ "Rust code ชุดนี้บน CPU นี้" ใช้ประกอบการตัดสินใจ ไม่ควรใช้ตัวเดียว ควรทดสอบกับ workload จริงของบริษัทด้วย
- **Binary มี 3 แบบ**: `x86_64` (baseline SSE2), `x86_64-v3` (AVX2/FMA), `aarch64` (NEON ซึ่งอยู่ใน baseline)
  Kunpeng 920 ไม่มี SVE (NEON 128 บิตเท่านั้น) จึงควรเทียบกับ `x86_64-v3` ด้วย และรายงานบันทึก compile-time features ไว้
  ถ้าเครื่องปลายทางมี Rust ติดตั้งอยู่ จะ build ด้วย `RUSTFLAGS="-C target-cpu=native"` ก็ได้ (`compare` จะแจ้งว่าเป็น native build)
- Crypto ใช้ RustCrypto crates: บน x86 ที่ **ไม่มี SHA-NI** (ก่อน Ice Lake) SHA-256 จะเป็น software ซึ่งอาจช้ากว่า OpenSSL (asm AVX2)
  ตัวเลขนี้จึงอาจทำให้ ARM ดูดีเกินจริงในส่วน SHA-256 — อ่านป้าย backend ในรายงานประกอบ
- **SHA-512 ไม่เป็นกลางระหว่างสถาปัตยกรรม**: Kunpeng 920 ไม่มี SHA512 extension จึงรัน software แบบ scalar ขณะที่ x86 ได้ implementation ที่จูนด้วย AVX2
  `crypto.sha512` จึงเอนเอียงเข้าข้าง x86 มากกว่าช่องว่างของฮาร์ดแวร์จริง (OpenSSL บน aarch64 จะเร็วกว่านี้) ป้าย backend ในรายงานบอกข้อนี้ต่อการรัน
- **ตัวเลขกลุ่ม allocator/memcpy ในชุด musl ต่ำกว่า glibc อย่างมีนัยสำคัญ** (วัดบนเครื่อง dev เดียวกัน, 1 thread, ทำซ้ำ 2 ครั้ง, glibc → musl):
  `mem.memcpy_small` 71 → 12 GB/s (ช้ากว่า ~6 เท่า), `comp.inflate` 365 → 198 MB/s, `data.json` 643 → 345 MB/s, `data.alloc` 55 → 33 Mmalloc/s
  kernel กลุ่มนี้ถูกติดป้าย `[libc-sensitive]` ในรายงาน และรายงานของ build musl จะมีคำเตือนระบุรายชื่อ kernel เหล่านั้น
  เทียบสองเครื่องด้วย musl เหมือนกันไม่ได้แปลว่ายุติธรรมเสมอไป เพราะ memcpy ของ musl จูนต่างกันแต่ละสถาปัตยกรรม (ยังไม่ได้วัดฝั่ง ARM ด้วย glibc)
  ถ้าต้องการตัวเลขที่ใกล้งานจริง ให้ build glibc จาก source บนเครื่องนั้น (หัวข้อด้านบน) และเทียบกลุ่มนี้จาก build glibc
- `data.alloc` ไม่ถูกวัดแบบ multi-thread เพราะ musl malloc ใช้ global lock ผลจะวัด lock ไม่ใช่ CPU
- `--cost-*` และ `--watts-*` เป็นค่าที่ผู้ใช้ป้อน ไม่ได้วัดจริง
- คะแนนรวม (1000 = 1 thread ของเครื่องอ้างอิง Ryzen AI 9 HX 370 ภายใต้ WSL2) ใช้เทียบเฉพาะรายงานจาก cpubench เวอร์ชันเดียวกัน
  ตัวเลขต่อ kernel และ ratio จาก `compare` คือหลักฐานหลัก
- **สถานะการยืนยัน**: binary aarch64 ถูกรันจริงบน Raspberry Pi 5 (Cortex-A76) ด้วย profile quick แบบ 1 thread (ครบ 22 kernel) และ multi-thread แบบย่อ:
  `info` ตรวจพบ CPU/cache/ISA ถูกต้อง, **checksum ตรงกับเครื่อง x86_64 ทั้ง 22 kernel** (รวม kernel ทศนิยม, JSON, DEFLATE)
  **ยังไม่เคยรัน profile full ครบชุดที่ไหนเลย** และยังไม่เคยรันบน Kunpeng 920 จริง (โค้ดถอดรหัส HiSilicon ทดสอบด้วยข้อความ /proc/cpuinfo ตัวอย่างเท่านั้น)
  รอบแรกบน TaiShan ให้รัน `cpubench-aarch64 info` ก่อน แล้วตรวจว่า CPU / จำนวน socket / NUMA / cache ถูกต้อง จากนั้นค่อยรัน full

## โครงสร้างโค้ด

```
src/main.rs        CLI (run / serve / compare / report / info / list)
src/serve/         เว็บ UI: server, ตัวจัดการ process ลูก, ตรวจ input, ui.html + app.js
src/harness.rs     วัดผล: barrier, warm-up, sample, รวมผลหลาย thread
src/kernels/       int, fp, mem, crypto, compress, data
src/sysinfo.rs     ตรวจสเปก (x86 + ARM/HiSilicon), cache, NUMA, topology
src/score.rs       คะแนนรวม + ค่าอ้างอิง
src/report/        โมเดลเอกสารกลาง + renderer text/md/html + compare
build.rs           บันทึก target/rustflags/rustc/libc ลงรายงาน
```

`make test` รัน unit test 80 ข้อ (ค่าคำตอบที่ทราบ เช่น จำนวนจำนวนเฉพาะ ≤10⁷, พลังงาน n-body, FIPS-197 AES, SHA test vector)
