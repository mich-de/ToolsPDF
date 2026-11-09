//! Data di compilazione per la schermata Info, senza dipendenze.

use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    let days = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() / 86_400) as i64;
    let (year, month, day) = civil_from_days(days);
    println!("cargo:rustc-env=TOOLSPDF_BUILD_DATE={day:02}/{month:02}/{year}");
    println!("cargo:rerun-if-changed=src");
}

/// Giorni dal 1970-01-01 a data del calendario gregoriano (algoritmo di H. Hinnant).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}
