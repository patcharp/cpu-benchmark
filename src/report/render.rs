//! Renderers for `Doc`: terminal text, Markdown, self-contained HTML.

use super::{Align, Block, Doc, Table};

fn width(s: &str) -> usize {
    s.chars().count()
}

fn pad(s: &str, w: usize, a: Align) -> String {
    let gap = w.saturating_sub(width(s));
    match a {
        Align::Left => format!("{s}{}", " ".repeat(gap)),
        Align::Right => format!("{}{s}", " ".repeat(gap)),
    }
}

const BAR_WIDTH: usize = 40;
/// Ratios within 2% of 1.00x are shown neutral, not as a loss.
const NEUTRAL_BAND: f64 = 0.98;

/// Bar length for a ratio on a log scale centred at 1.0 (so 0.5x and 2x are
/// mirror images) with `max` taking the full width.
fn bar_len(ratio: f64, max: f64) -> usize {
    if !ratio.is_finite() || ratio <= 0.0 {
        return 0;
    }
    let max = max.max(1.0 / max.max(1e-9)).max(1.0001);
    let t = (ratio.ln() / max.ln() * 0.5 + 0.5).clamp(0.0, 1.0);
    (t * BAR_WIDTH as f64).round() as usize
}

fn bar_max(items: &[(String, f64)]) -> f64 {
    items
        .iter()
        .map(|(_, r)| if *r >= 1.0 { *r } else { 1.0 / r })
        .filter(|r| r.is_finite())
        .fold(1.5, f64::max)
}

// ---------------------------------------------------------------- text

pub fn text(doc: &Doc) -> String {
    let mut out = format!("{}\n{}\n{}\n", doc.title, "=".repeat(width(&doc.title)), doc.subtitle);
    for b in &doc.blocks {
        out.push('\n');
        match b {
            Block::Heading(h) => out.push_str(&format!("{h}\n{}\n", "-".repeat(width(h)))),
            Block::Para(p) => out.push_str(&format!("{p}\n")),
            Block::Warnings(items) => items.iter().for_each(|i| out.push_str(&format!("  [!] {i}\n"))),
            Block::Table(t) => out.push_str(&text_table(t)),
            Block::Bars { title, items } => {
                out.push_str(&format!("{title}  (| marks 1.00x = equal)\n"));
                let max = bar_max(items);
                let lw = items.iter().map(|(l, _)| width(l)).max().unwrap_or(0);
                let mid = bar_len(1.0, max);
                for (label, r) in items {
                    let n = bar_len(*r, max);
                    let mut bar: Vec<char> = (0..BAR_WIDTH).map(|i| if i < n { '#' } else { ' ' }).collect();
                    if mid < BAR_WIDTH {
                        bar[mid] = '|';
                    }
                    out.push_str(&format!("  {}  {}  {:>6.2}x\n", pad(label, lw, Align::Left), bar.iter().collect::<String>(), r));
                }
            }
        }
    }
    out
}

fn text_table(t: &Table) -> String {
    let widths: Vec<usize> = (0..t.headers.len())
        .map(|c| t.rows.iter().map(|r| width(&r[c])).chain([width(&t.headers[c])]).max().unwrap_or(0))
        .collect();
    let line = |cells: &[String]| {
        let parts: Vec<String> = cells.iter().enumerate().map(|(i, c)| pad(c, widths[i], t.align[i])).collect();
        format!("  {}\n", parts.join("  ").trim_end())
    };
    let mut s = line(&t.headers);
    s.push_str(&format!("  {}\n", widths.iter().map(|w| "-".repeat(*w)).collect::<Vec<_>>().join("  ")));
    t.rows.iter().for_each(|r| s.push_str(&line(r)));
    s
}

// ------------------------------------------------------------ markdown

fn md_escape(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

pub fn markdown(doc: &Doc) -> String {
    let mut out = format!("# {}\n\n_{}_\n", doc.title, doc.subtitle);
    for b in &doc.blocks {
        out.push('\n');
        match b {
            Block::Heading(h) => out.push_str(&format!("## {h}\n")),
            Block::Para(p) => out.push_str(&format!("{p}\n")),
            Block::Warnings(items) => {
                items.iter().for_each(|i| out.push_str(&format!("> **Warning:** {i}\n>\n")));
            }
            Block::Table(t) => out.push_str(&md_table(t)),
            Block::Bars { title, items } => {
                let max = bar_max(items);
                let mut t = Table::new(&[title.as_str(), "Ratio", ""], &[Align::Left, Align::Right, Align::Left]);
                for (label, r) in items {
                    t.row([label.clone(), format!("{r:.2}x"), "█".repeat(bar_len(*r, max) / 2)]);
                }
                out.push_str(&md_table(&t));
            }
        }
    }
    out
}

fn md_table(t: &Table) -> String {
    let mut s = format!("| {} |\n", t.headers.iter().map(|h| md_escape(h)).collect::<Vec<_>>().join(" | "));
    s.push_str(&format!(
        "|{}|\n",
        t.align.iter().map(|a| if *a == Align::Right { "---:" } else { "---" }).collect::<Vec<_>>().join("|")
    ));
    t.rows.iter().for_each(|r| {
        s.push_str(&format!("| {} |\n", r.iter().map(|c| md_escape(c)).collect::<Vec<_>>().join(" | ")))
    });
    s
}

// ---------------------------------------------------------------- html

pub fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

const CSS: &str = r#"
:root{--bg:#fff;--fg:#1c2330;--muted:#5b6678;--line:#dfe3ea;--head:#f3f5f9;--warn-bg:#fff4e0;--warn-line:#e8a33d;--bar:#3b82f6;--bar-neg:#e5484d;--mark:#1c2330}
@media (prefers-color-scheme:dark){:root{--bg:#12161d;--fg:#e6e9ef;--muted:#9aa4b5;--line:#2a3140;--head:#1a2029;--warn-bg:#2b2213;--warn-line:#b9812c;--bar:#5b9bff;--bar-neg:#ff6b70;--mark:#e6e9ef}}
*{box-sizing:border-box}
body{margin:0 auto;max-width:1100px;padding:24px 16px 64px;background:var(--bg);color:var(--fg);font:15px/1.5 system-ui,-apple-system,"Segoe UI",sans-serif}
h1{font-size:1.7rem;margin:0 0 4px}h2{font-size:1.2rem;margin:32px 0 10px;padding-bottom:4px;border-bottom:1px solid var(--line)}
.sub{color:var(--muted);margin:0 0 8px}
.table-wrap{overflow-x:auto}
table{border-collapse:collapse;width:100%;font-size:.9rem}
th,td{padding:5px 10px;border-bottom:1px solid var(--line);vertical-align:top}
th{background:var(--head);text-align:left;font-weight:600}
td.r,th.r{text-align:right;font-variant-numeric:tabular-nums;white-space:nowrap}
.warn{background:var(--warn-bg);border-left:4px solid var(--warn-line);padding:8px 12px;margin:6px 0;border-radius:4px}
.bars{display:grid;grid-template-columns:minmax(120px,max-content) 1fr 70px;gap:4px 12px;align-items:center;font-size:.9rem}
.track{position:relative;height:14px;background:var(--head);border-radius:3px}
.fill{position:absolute;left:0;top:0;bottom:0;border-radius:3px;background:var(--bar)}
.fill.neg{background:var(--bar-neg)}
.mark{position:absolute;top:-3px;bottom:-3px;width:2px;background:var(--mark)}
.ratio{text-align:right;font-variant-numeric:tabular-nums}
ul{padding-left:20px}
@media print{body{max-width:none}}
"#;

pub fn html(doc: &Doc) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{}</title><style>{CSS}</style></head><body>\n<h1>{}</h1>\n<p class=\"sub\">{}</p>\n",
        html_escape(&doc.title),
        html_escape(&doc.title),
        html_escape(&doc.subtitle)
    ));
    for b in &doc.blocks {
        match b {
            Block::Heading(h) => out.push_str(&format!("<h2>{}</h2>\n", html_escape(h))),
            Block::Para(p) => out.push_str(&format!("<p>{}</p>\n", html_escape(p))),
            Block::Warnings(items) => items
                .iter()
                .for_each(|i| out.push_str(&format!("<div class=\"warn\"><strong>Warning:</strong> {}</div>\n", html_escape(i)))),
            Block::Table(t) => out.push_str(&html_table(t)),
            Block::Bars { title, items } => {
                out.push_str(&format!("<p><strong>{}</strong> <span class=\"sub\">(black marker = 1.00x, equal)</span></p>\n<div class=\"bars\">", html_escape(title)));
                let max = bar_max(items);
                let mid = bar_len(1.0, max) as f64 / BAR_WIDTH as f64 * 100.0;
                for (label, r) in items {
                    let pct = bar_len(*r, max) as f64 / BAR_WIDTH as f64 * 100.0;
                    out.push_str(&format!(
                        "<div>{}</div><div class=\"track\"><div class=\"fill{}\" style=\"width:{pct:.1}%\"></div><div class=\"mark\" style=\"left:{mid:.1}%\"></div></div><div class=\"ratio\">{r:.2}x</div>",
                        html_escape(label),
                        if *r < NEUTRAL_BAND { " neg" } else { "" },
                    ));
                }
                out.push_str("</div>\n");
            }
        }
    }
    out.push_str("</body></html>\n");
    out
}

fn html_table(t: &Table) -> String {
    let cls = |a: Align| if a == Align::Right { " class=\"r\"" } else { "" };
    let mut s = String::from("<div class=\"table-wrap\"><table><thead><tr>");
    t.headers.iter().enumerate().for_each(|(i, h)| s.push_str(&format!("<th{}>{}</th>", cls(t.align[i]), html_escape(h))));
    s.push_str("</tr></thead><tbody>");
    for r in &t.rows {
        s.push_str("<tr>");
        r.iter().enumerate().for_each(|(i, c)| s.push_str(&format!("<td{}>{}</td>", cls(t.align[i]), html_escape(c))));
        s.push_str("</tr>");
    }
    s.push_str("</tbody></table></div>\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Doc {
        let mut t = Table::new(&["Name", "Value"], &[Align::Left, Align::Right]);
        t.row(["a|b", "1.00"]);
        t.row(["<script>", "22.50"]);
        Doc {
            title: "T".into(),
            subtitle: "S & S".into(),
            blocks: vec![
                Block::Heading("H".into()),
                Block::Warnings(vec!["careful".into()]),
                Block::Table(t),
                Block::Bars { title: "ratios".into(), items: vec![("fast".into(), 2.0), ("slow".into(), 0.5)] },
            ],
        }
    }

    #[test]
    fn text_contains_everything() {
        let t = text(&sample());
        assert!(t.contains("[!] careful") && t.contains("22.50") && t.contains("2.00x") && t.contains("0.50x"));
    }

    #[test]
    fn markdown_escapes_pipes() {
        let m = markdown(&sample());
        assert!(m.contains("a\\|b") && m.contains("> **Warning:** careful") && m.contains("---:"));
    }

    #[test]
    fn html_escapes_content() {
        let h = html(&sample());
        assert!(h.contains("&lt;script&gt;") && !h.contains("<script>") && h.contains("S &amp; S"));
        assert!(h.starts_with("<!doctype html>"));
    }

    #[test]
    fn bars_are_symmetric_on_log_scale() {
        let max = 4.0;
        let (up, down, mid) = (bar_len(2.0, max), bar_len(0.5, max), bar_len(1.0, max));
        assert_eq!(up - mid, mid - down);
        assert_eq!(bar_len(f64::NAN, max), 0);
    }
}
