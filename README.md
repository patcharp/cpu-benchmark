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

## Build จาก source บน OS เป้าหมาย (เลือก glibc หรือ musl เองได้)

`make src-dist` สร้าง `dist/cpubench-src.tar.gz` (≈6 MB, รวม dependency ที่ vendor ไว้แล้ว) เครื่องปลายทางต้องมีแค่ Rust toolchain
(ใช้ rustup หรือตัวติดตั้ง offline จาก static.rust-lang.org; dependency ตัวหนึ่งประกาศ rust-version 1.89 จึงแนะนำ Rust ≥ 1.89) ไม่ต้องใช้เน็ต

```bash
tar xzf cpubench-src.tar.gz && cd cpubench

# glibc (ใช้ libc ของ OS นั้น = สิ่งที่บริการจริงใช้)  -> ./target/release/cpubench
cargo build --release --offline

# musl static (ต้อง rustup target add <arch>-unknown-linux-musl ก่อน) -> ./target/<arch>-unknown-linux-musl/release/cpubench
cargo build --release --offline --target aarch64-unknown-linux-musl     # หรือ x86_64-unknown-linux-musl

# ใช้ชุดคำสั่งเต็มของ CPU นั้น (AVX-512 / ฯลฯ); รายงานจะบันทึกไว้ว่าเป็น native build
RUSTFLAGS="-C target-cpu=native" cargo build --release --offline
```

รายงานบันทึก libc ของ build (`glibc` / `musl`) และ `compare` จะเตือนถ้าสองรายงานใช้ libc ไม่ตรงกัน
(ถ้า `rust-lld` ไม่มีบนเครื่องปลายทาง ให้ลบบรรทัด `linker = "rust-lld"` ใน `.cargo/config.toml` สำหรับ target musl ที่ไม่ได้ใช้)

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
src/main.rs        CLI (run / compare / report / info / list)
src/harness.rs     วัดผล: barrier, warm-up, sample, รวมผลหลาย thread
src/kernels/       int, fp, mem, crypto, compress, data
src/sysinfo.rs     ตรวจสเปก (x86 + ARM/HiSilicon), cache, NUMA, topology
src/score.rs       คะแนนรวม + ค่าอ้างอิง
src/report/        โมเดลเอกสารกลาง + renderer text/md/html + compare
build.rs           บันทึก target/rustflags/rustc/libc ลงรายงาน
```

`make test` รัน unit test 63 ข้อ (ค่าคำตอบที่ทราบ เช่น จำนวนจำนวนเฉพาะ ≤10⁷, พลังงาน n-body, FIPS-197 AES, SHA test vector)
