//! Microbench for `TEXT` formatting hot paths.
//!
//! ```text
//! cargo run -p xlsx-engine-core --release --example text_bench
//! ```

use std::time::Instant;
use xlsx_engine_core::text_format::{apply, apply_generic, apply_naive, clear_plan_cache};
use xlsx_types::{DateSystem, ExcelValue};

const ITERS: u32 = 200_000;
const WARMUP: u32 = 20_000;

fn nsec_per(iters: u32, f: impl Fn()) -> f64 {
    for _ in 0..WARMUP {
        f();
    }
    let t = Instant::now();
    for _ in 0..iters {
        f();
    }
    t.elapsed().as_nanos() as f64 / iters as f64
}

fn bench(label: &str, value: ExcelValue, fmt: &str) {
    let naive = nsec_per(ITERS, || {
        let _ = apply_naive(&value, fmt, DateSystem::Excel1900);
    });
    clear_plan_cache();
    let generic_cold = nsec_per(ITERS, || {
        let _ = apply_generic(&value, fmt, DateSystem::Excel1900);
    });
    let generic_hot = nsec_per(ITERS, || {
        let _ = apply_generic(&value, fmt, DateSystem::Excel1900);
    });
    clear_plan_cache();
    let with_fast = nsec_per(ITERS, || {
        let _ = apply(&value, fmt, DateSystem::Excel1900);
    });
    let speedup = naive / with_fast;
    println!(
        "{label:<22} naive {naive:7.1} ns   generic-cold {generic_cold:7.1} ns   generic-hot {generic_hot:7.1} ns   apply {with_fast:7.1} ns   ×{speedup:.2}"
    );
}

fn main() {
    println!("TEXT microbench  iters={ITERS} warmup={WARMUP} (release)");
    bench("0.00", ExcelValue::Number(1234.567), "0.00");
    bench("#,##0", ExcelValue::Number(1_234_567.0), "#,##0");
    bench("#,##0.00", ExcelValue::Number(1234.567), "#,##0.00");
    bench("0%", ExcelValue::Number(0.125), "0%");
    bench("0.00%", ExcelValue::Number(0.285), "0.00%");
    bench("yyyy-mm-dd", ExcelValue::Number(45366.0), "yyyy-mm-dd");
    bench("$#,##0.00", ExcelValue::Number(1234.567), "$#,##0.00");
    bench("non-numeric", ExcelValue::Text("abc".into()), "0.00");
    bench("0000000", ExcelValue::Number(1234.0), "0000000");
    bench("#.#", ExcelValue::Number(0.5), "#.#");
    bench("@", ExcelValue::Number(1234.5), "@");
    bench("mm/dd/yyyy", ExcelValue::Number(45366.0), "mm/dd/yyyy");
}
