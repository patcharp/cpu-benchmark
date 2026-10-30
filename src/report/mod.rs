//! Reports are built once as a small format-neutral `Doc` (headings, tables,
//! bars...) and rendered to terminal text, Markdown and self-contained HTML,
//! so the three outputs can never disagree.

pub mod compare;
pub mod render;
pub mod run;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Align {
    Left,
    Right,
}

#[derive(Clone, Debug, Default)]
pub struct Table {
    pub headers: Vec<String>,
    pub align: Vec<Align>,
    pub rows: Vec<Vec<String>>,
}

#[derive(Clone, Debug)]
pub enum Block {
    Heading(String),
    Para(String),
    /// Findings the reader must not miss (caveats, fallbacks, mismatches).
    Warnings(Vec<String>),
    Table(Table),
    /// Ratios where 1.0 means "equal"; bigger is better for the candidate.
    Bars { title: String, items: Vec<(String, f64)> },
}

#[derive(Clone, Debug, Default)]
pub struct Doc {
    pub title: String,
    pub subtitle: String,
    pub blocks: Vec<Block>,
}

impl Table {
    pub fn new(headers: &[&str], align: &[Align]) -> Self {
        assert_eq!(headers.len(), align.len());
        Table { headers: headers.iter().map(|s| s.to_string()).collect(), align: align.to_vec(), rows: Vec::new() }
    }
    pub fn row<S: Into<String>>(&mut self, cells: impl IntoIterator<Item = S>) {
        let row: Vec<String> = cells.into_iter().map(Into::into).collect();
        assert_eq!(row.len(), self.headers.len(), "row width must match header");
        self.rows.push(row);
    }
}

/// Number with a precision that scales with magnitude.
pub fn fmt_num(v: f64) -> String {
    if !v.is_finite() {
        "n/a".into()
    } else if v.abs() >= 1000.0 {
        format!("{v:.0}")
    } else if v.abs() >= 100.0 {
        format!("{v:.1}")
    } else if v.abs() >= 1.0 {
        format!("{v:.2}")
    } else {
        format!("{v:.3}")
    }
}

pub fn fmt_ratio(r: f64) -> String {
    if r.is_finite() { format!("{r:.2}x") } else { "n/a".into() }
}

pub fn fmt_bytes(b: u64) -> String {
    const U: [(&str, u64); 4] = [("GiB", 1 << 30), ("MiB", 1 << 20), ("KiB", 1 << 10), ("B", 1)];
    for (name, size) in U {
        if b >= size {
            let v = b as f64 / size as f64;
            return if (v - v.round()).abs() < 0.05 { format!("{} {name}", v.round() as u64) } else { format!("{v:.1} {name}") };
        }
    }
    "0 B".into()
}

pub fn or_unknown<T: ToString>(v: Option<T>) -> String {
    v.map_or_else(|| "unknown".to_string(), |x| x.to_string())
}

#[cfg(test)]
pub(crate) mod testutil {
    use crate::model::*;
    use crate::stats::summarize;

    pub fn sample_report() -> Report {
        let kr = |id: &str, hib: bool, st: f64, mt: Option<f64>| KernelResult {
            id: id.into(),
            category: if id.starts_with("mem") { "Memory".into() } else { "Integer".into() },
            description: "d".into(),
            unit: "u".into(),
            higher_is_better: hib,
            status: STATUS_OK.into(),
            note: None,
            libc_sensitive: false,
            backend: None,
            single: Some(summarize(1, &[st, st * 1.01, st * 0.99])),
            multi: mt.map(|m| summarize(8, &[m, m * 1.2, m * 0.8])),
            checksum: "0000000000000001".into(),
            checksum_consistent: true,
        };
        Report {
            schema: SCHEMA,
            tool_version: "0.1.0".into(),
            label: "unit".into(),
            started_unix: 0,
            elapsed_secs: 1.0,
            profile: "quick".into(),
            config: RunConfig { threads: 8, pinned: false, cpus: vec![], cores_used: 8, warmup_secs: 0.1, sample_secs: 0.1, samples: 3, mem_budget_bytes: 1 << 30 },
            system: SysInfo { arch: "aarch64".into(), logical_cpus: 8, usable_parallelism: 8, page_size: 65536, features: vec!["asimd".into(), "sha2".into(), "aes".into()], ..Default::default() },
            build: BuildInfo::default(),
            results: vec![kr("int.a", true, 100.0, Some(700.0)), kr("mem.lat.2m", false, 5.0, None)],
            scores: Scores::default(),
            warnings: vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_formatting() {
        assert_eq!(fmt_num(12345.6), "12346");
        assert_eq!(fmt_num(123.456), "123.5");
        assert_eq!(fmt_num(12.345), "12.35");
        assert_eq!(fmt_num(0.1234), "0.123");
        assert_eq!(fmt_num(f64::NAN), "n/a");
        assert_eq!(fmt_ratio(1.234), "1.23x");
    }

    #[test]
    fn byte_formatting() {
        assert_eq!(fmt_bytes(64 << 10), "64 KiB");
        assert_eq!(fmt_bytes(3 << 29), "1.5 GiB");
        assert_eq!(fmt_bytes(0), "0 B");
    }

    #[test]
    #[should_panic]
    fn table_rejects_ragged_rows() {
        let mut t = Table::new(&["a", "b"], &[Align::Left, Align::Left]);
        t.row(["only one"]);
    }
}
