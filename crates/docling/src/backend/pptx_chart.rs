//! A PPTX chart part read the way docling's PowerPoint backend reads it
//! through python-pptx (docling#4773, 2.139): every plot of a combination
//! chart, not just the first.
//!
//! - **Plots**: the plot area's chart elements in document order. python-pptx
//!   reads nine of them; the rest (3-D bar/line/pie, stock, surface,
//!   bar-of-pie) are plots it "cannot read": they count as `other_chart` and
//!   add no data, while the other plots keep theirs.
//! - **Classification**: one label per plot type in drawing order (the
//!   back-most plot first, so it stays the main prediction), each label once.
//! - **Data table**: the series of every readable plot as columns, in drawing
//!   order and each plot's `c:order`. The categories come from the first
//!   plot that has them; on a date axis, or a text axis showing a date number
//!   format, the cached serials read as ISO 8601 dates (1900 or 1904 system).
//!   That plot's category axis title heads the category column.
//! - **Title**: `c:chart/c:title`'s rich text only — an axis title is not the
//!   chart's.
//!
//! The XLSX and DOCX backends keep their own chart readers
//! ([`super::xlsx_drawings`]): their upstream counterparts did not change.

use roxmltree::{Document, Node as XmlNode};

use super::numfmt::{civil_from_days, days_from_civil};
use super::xlsx_drawings::chart_table;

/// A PPTX chart as docling records it on the picture.
pub struct PptxChart {
    /// The classification labels, main first (never empty).
    pub labels: Vec<&'static str>,
    /// The chart title, the picture's caption.
    pub title: Option<String>,
    /// The data grid; `None` when no readable plot has a series or no row
    /// carries data.
    pub table: Option<docling_core::Table>,
}

/// The plot elements python-pptx lists (`CT_PlotArea.iter_xCharts`).
const PLOT_TAGS: [&str; 16] = [
    "area3DChart",
    "areaChart",
    "bar3DChart",
    "barChart",
    "bubbleChart",
    "doughnutChart",
    "line3DChart",
    "lineChart",
    "ofPieChart",
    "pie3DChart",
    "pieChart",
    "radarChart",
    "scatterChart",
    "stockChart",
    "surface3DChart",
    "surfaceChart",
];

/// Backstop on the cells of one chart's table (docling's
/// `_MAX_CHART_TABLE_CELLS`).
const MAX_CHART_TABLE_CELLS: usize = 100_000;

/// The label of a plot python-pptx can read (its `PlotFactory`), by the
/// family of the type `PlotTypeInspector` gives it; `None` for a plot it
/// cannot read.
fn readable_plot_label(tag: &str) -> Option<&'static str> {
    Some(match tag {
        "barChart" => "bar_chart",
        "lineChart" => "line_chart",
        "pieChart" | "doughnutChart" => "pie_chart",
        "scatterChart" => "scatter_chart",
        // AREA…, THREE_D_AREA…, BUBBLE…, RADAR…
        "areaChart" | "area3DChart" | "bubbleChart" | "radarChart" => "other_chart",
        _ => return None,
    })
}

fn child<'a, 'i>(node: XmlNode<'a, 'i>, tag: &str) -> Option<XmlNode<'a, 'i>> {
    node.children().find(|c| c.has_tag_name(tag))
}

/// Parse a chart part; `None` when it is not a chart (no plot area).
pub fn parse(xml: &str) -> Option<PptxChart> {
    let dom = Document::parse(xml).ok()?;
    let chart = child(dom.root_element(), "chart")?;
    let plot_area = child(chart, "plotArea")?;
    let date1904 = child(dom.root_element(), "date1904")
        .and_then(|d| d.attribute("val"))
        .is_some_and(|v| matches!(v, "1" | "true"));
    let plots: Vec<XmlNode> = plot_area
        .children()
        .filter(|c| c.is_element() && PLOT_TAGS.contains(&c.tag_name().name()))
        .collect();

    let mut labels: Vec<&'static str> = Vec::new();
    for plot in &plots {
        let label = readable_plot_label(plot.tag_name().name()).unwrap_or("other_chart");
        if !labels.contains(&label) {
            labels.push(label);
        }
    }
    if labels.is_empty() {
        labels.push("other_chart");
    }

    Some(PptxChart {
        labels,
        title: child(chart, "title").and_then(chart_title),
        table: chart_data(plot_area, &plots, date1904),
    })
}

/// python-pptx's `chart_title.text_frame.text`, stripped: the rich text's
/// paragraphs joined with newlines (a line break is `\v`).
fn chart_title(title: XmlNode) -> Option<String> {
    let rich = child(title, "tx").and_then(|tx| child(tx, "rich"))?;
    let text = rich
        .children()
        .filter(|p| p.has_tag_name("p"))
        .map(|p| {
            let mut s = String::new();
            for el in p.children().filter(XmlNode::is_element) {
                match el.tag_name().name() {
                    "r" | "fld" => {
                        if let Some(t) = child(el, "t") {
                            s.push_str(t.text().unwrap_or(""));
                        }
                    }
                    "br" => s.push('\u{b}'),
                    _ => {}
                }
            }
            s
        })
        .collect::<Vec<_>>()
        .join("\n");
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// docling's `_chart_to_table_data`.
fn chart_data(
    plot_area: XmlNode,
    plots: &[XmlNode],
    date1904: bool,
) -> Option<docling_core::Table> {
    let mut categories: Vec<String> = Vec::new();
    let mut corner = String::new();
    let mut columns: Vec<(String, Vec<String>)> = Vec::new();
    for &plot in plots {
        if readable_plot_label(plot.tag_name().name()).is_none() {
            continue;
        }
        if categories.is_empty() {
            let axis = category_axis(plot_area, plot);
            categories = category_labels(plot, axis, date1904);
            corner = axis.map(axis_title).unwrap_or_default();
        }
        let mut sers: Vec<XmlNode> = plot.children().filter(|c| c.has_tag_name("ser")).collect();
        sers.sort_by_key(|s| {
            child(*s, "order")
                .and_then(|o| o.attribute("val"))
                .and_then(|v| v.parse::<i64>().ok())
                .unwrap_or(0)
        });
        for ser in sers {
            let name = ser
                .children()
                .filter(|c| c.has_tag_name("tx"))
                .flat_map(|tx| tx.descendants())
                .find(|n| n.has_tag_name("pt"))
                .and_then(|pt| child(pt, "v"))
                .and_then(|v| v.text())
                .unwrap_or("")
                .to_string();
            // A value that is not a number fails python-pptx's `float()`,
            // and docling then drops the whole table.
            columns.push((name, series_values(ser)?));
        }
    }
    if columns.is_empty() {
        return None;
    }
    // Trailing rows with no category and no value are dropped (interior gaps
    // stay), then the table is capped.
    let declared = columns
        .iter()
        .map(|(_, v)| v.len())
        .chain([categories.len()])
        .max()
        .unwrap_or(0);
    let populated = (0..declared)
        .rev()
        .find(|&row| {
            categories.get(row).is_some_and(|c| !c.is_empty())
                || columns
                    .iter()
                    .any(|(_, v)| v.get(row).is_some_and(|s| !s.is_empty()))
        })
        .map_or(0, |row| row + 1);
    if populated == 0 {
        return None;
    }
    let num_cols = 1 + columns.len();
    let rows = if populated * num_cols > MAX_CHART_TABLE_CELLS {
        (MAX_CHART_TABLE_CELLS / num_cols).max(1)
    } else {
        populated
    };
    categories.truncate(rows);
    categories.resize(rows, String::new());
    for (_, values) in &mut columns {
        values.truncate(rows);
    }
    chart_table(categories, columns, corner)
}

/// A series' values as python-pptx reads them (`c:val`, else `c:yVal`): one
/// per point of the cache's `c:ptCount`, empty where a point is missing,
/// integer-valued numbers without `.0` (docling's `_cell_text`). `None` when
/// a point is not a number.
fn series_values(ser: XmlNode) -> Option<Vec<String>> {
    let Some(val) = child(ser, "val").or_else(|| child(ser, "yVal")) else {
        return Some(Vec::new());
    };
    let count = val
        .descendants()
        .find(|n| n.has_tag_name("ptCount"))
        .and_then(|n| n.attribute("val"))
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0);
    let mut values = vec![String::new(); count];
    for pt in val.descendants().filter(|n| n.has_tag_name("pt")) {
        let Some(idx) = pt.attribute("idx").and_then(|i| i.parse::<usize>().ok()) else {
            continue;
        };
        if idx >= count {
            continue;
        }
        let text = child(pt, "v").and_then(|v| v.text()).unwrap_or("");
        let value: f64 = text.trim().parse().ok()?;
        values[idx] = python_number(value);
    }
    Some(values)
}

/// A float as docling's `_cell_text` writes it: an integer value without
/// `.0`, anything else as Python's `str(float)`.
fn python_number(v: f64) -> String {
    if v.is_nan() {
        return "nan".into();
    }
    if v.is_infinite() {
        return if v > 0.0 { "inf" } else { "-inf" }.into();
    }
    if v.fract() == 0.0 {
        return if v == 0.0 {
            "0".into()
        } else {
            format!("{v:.0}")
        };
    }
    // Python's repr switches to scientific notation below 1e-4 (and from
    // 1e16, where every float is an integer), with a signed two-digit
    // exponent.
    let sci = format!("{v:e}");
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    if exp < -4 {
        let sign = if exp < 0 { '-' } else { '+' };
        format!("{mantissa}e{sign}{:02}", exp.abs())
    } else {
        format!("{v}")
    }
}

/// The `c:catAx`/`c:dateAx` a plot is drawn against, matched by `c:axId`
/// (docling's `_category_axis`); `None` for a plot without one (pie, xy).
fn category_axis<'a, 'i>(plot_area: XmlNode<'a, 'i>, plot: XmlNode) -> Option<XmlNode<'a, 'i>> {
    let ids: Vec<&str> = plot
        .children()
        .filter(|c| c.has_tag_name("axId"))
        .filter_map(|c| c.attribute("val"))
        .collect();
    plot_area.children().find(|axis| {
        (axis.has_tag_name("catAx") || axis.has_tag_name("dateAx"))
            && axis
                .children()
                .filter(|c| c.has_tag_name("axId"))
                .filter_map(|c| c.attribute("val"))
                .any(|id| ids.contains(&id))
    })
}

/// A plot's category labels (python-pptx's `plot.categories`: the first
/// series' `c:cat` points, first level only, padded to the cached count),
/// with dates as ISO 8601 when its axis shows dates.
fn category_labels(plot: XmlNode, axis: Option<XmlNode>, date1904: bool) -> Vec<String> {
    let first_cat = plot
        .children()
        .find(|c| c.has_tag_name("ser"))
        .and_then(|ser| child(ser, "cat"));
    let count = plot
        .children()
        .filter(|c| c.has_tag_name("ser"))
        .filter_map(|ser| child(ser, "cat"))
        .flat_map(|cat| cat.descendants())
        .find(|n| n.has_tag_name("ptCount"))
        .and_then(|n| n.attribute("val"))
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0);
    let mut labels = vec![String::new(); count];
    if let Some(cat) = first_cat {
        let first_level = cat.descendants().find(|n| n.has_tag_name("lvl"));
        let pts: Vec<XmlNode> = match first_level {
            Some(lvl) => lvl.children().filter(|c| c.has_tag_name("pt")).collect(),
            None => cat.descendants().filter(|n| n.has_tag_name("pt")).collect(),
        };
        for pt in pts {
            let Some(idx) = pt.attribute("idx").and_then(|i| i.parse::<usize>().ok()) else {
                continue;
            };
            if let Some(slot) = labels.get_mut(idx) {
                *slot = child(pt, "v")
                    .and_then(|v| v.text())
                    .unwrap_or("")
                    .to_string();
            }
        }
    }
    match axis {
        Some(axis) if categories_are_dates(axis, plot) => labels
            .into_iter()
            .map(|label| serial_to_iso(&label, date1904))
            .collect(),
        _ => labels,
    }
}

/// docling's `_categories_are_dates`: a date axis always; a text axis when
/// the number format it shows — its own, unless linked to the source data,
/// then the cached categories' — is a date format.
fn categories_are_dates(axis: XmlNode, plot: XmlNode) -> bool {
    if axis.has_tag_name("dateAx") {
        return true;
    }
    let own = child(axis, "numFmt")
        .filter(|f| !matches!(f.attribute("sourceLinked"), Some("1") | Some("true")));
    let format_code = match own {
        Some(f) => f.attribute("formatCode").unwrap_or("").to_string(),
        None => plot
            .children()
            .find(|c| c.has_tag_name("ser"))
            .and_then(|ser| child(ser, "cat"))
            .map(|cat| {
                cat.descendants()
                    .filter(|n| n.has_tag_name("numCache"))
                    .filter_map(|c| child(c, "formatCode"))
                    .filter_map(|f| f.text())
                    .collect::<String>()
            })
            .unwrap_or_default(),
    };
    is_date_format(&format_code)
}

/// docling's `_is_date_format`: with the literal parts removed (quoted text,
/// escaped characters, bracketed colours/locales/conditions, padding), a `d`
/// or `y` code is a date, and an `m` code is a month (not minutes) when
/// there is no `h` or `s`.
fn is_date_format(format_code: &str) -> bool {
    let mut codes = String::new();
    let mut chars = format_code.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                for q in chars.by_ref() {
                    if q == '"' {
                        break;
                    }
                }
            }
            '[' => {
                for q in chars.by_ref() {
                    if q == ']' {
                        break;
                    }
                }
            }
            '\\' | '_' | '*' => {
                chars.next();
            }
            _ => codes.extend(c.to_lowercase()),
        }
    }
    if codes.contains('d') || codes.contains('y') {
        return true;
    }
    codes.contains('m') && !codes.contains('h') && !codes.contains('s')
}

/// docling's `_serial_to_iso`: an Excel date serial as an ISO 8601 date (a
/// date and time to the second when it has a time of day), else the text
/// unchanged — not a number, negative, or out of Python's `datetime` range.
fn serial_to_iso(text: &str, date1904: bool) -> String {
    let Ok(serial) = text.trim().parse::<f64>() else {
        return text.to_string();
    };
    if !serial.is_finite() || serial < 0.0 {
        return text.to_string();
    }
    let epoch = if date1904 {
        days_from_civil(1904, 1, 1)
    } else {
        // Excel counts the nonexistent 1900-02-29 as serial 60.
        days_from_civil(1899, 12, 30) + i64::from(serial < 60.0)
    };
    // `timedelta(seconds=round(serial * 86400))`: Python rounds half to even.
    let seconds = (serial * 86_400.0).round_ties_even();
    if seconds > 1.0e15 {
        return text.to_string();
    }
    let seconds = seconds as i64;
    let days = epoch + seconds.div_euclid(86_400);
    let secs = seconds.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    if y > 9999 {
        return text.to_string();
    }
    if secs == 0 {
        format!("{y:04}-{m:02}-{d:02}")
    } else {
        format!(
            "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}",
            secs / 3600,
            secs / 60 % 60,
            secs % 60
        )
    }
}

/// docling's `_axis_title_text`: the rich text's paragraphs, each stripped,
/// joined with a space; else a cell-linked title's cached text.
fn axis_title(axis: XmlNode) -> String {
    let Some(tx) = child(axis, "title").and_then(|t| child(t, "tx")) else {
        return String::new();
    };
    let rich = child(tx, "rich")
        .map(|rich| {
            rich.children()
                .filter(|p| p.has_tag_name("p"))
                .map(|p| {
                    p.descendants()
                        .filter(|n| n.has_tag_name("t"))
                        .filter_map(|n| n.text())
                        .collect::<String>()
                        .trim()
                        .to_string()
                })
                .filter(|p| !p.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();
    let text = if rich.is_empty() {
        child(tx, "strRef")
            .map(|r| {
                r.descendants()
                    .filter(|n| n.has_tag_name("v"))
                    .filter_map(|n| n.text())
                    .collect::<String>()
            })
            .unwrap_or_default()
    } else {
        rich
    };
    text.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// docling's `test_serial_to_iso` cases.
    #[test]
    fn serials_map_to_iso_dates_in_both_systems() {
        for (text, date1904, want) in [
            ("40909", false, "2012-01-01"),
            ("40909.0", false, "2012-01-01"),
            ("39447", true, "2012-01-01"),
            ("40909.5", false, "2012-01-01T12:00:00"),
            ("1", false, "1900-01-01"),
            ("61", false, "1900-03-01"),
            ("Jan", false, "Jan"),
            ("", false, ""),
            ("-1", false, "-1"),
            ("1e300", false, "1e300"),
        ] {
            assert_eq!(serial_to_iso(text, date1904), want, "{text}");
        }
    }

    #[test]
    fn date_formats_are_told_from_time_and_general_ones() {
        for (code, want) in [
            ("m/d/yy", true),
            ("mmm d, yy", true),
            ("yyyy", true),
            ("mmm", true),
            ("h:mm", false),
            ("mm:ss", false),
            ("General", false),
            ("\"day\" 0", false),
            ("[Red]0.00", false),
            ("0_m", false),
        ] {
            assert_eq!(is_date_format(code), want, "{code}");
        }
    }

    #[test]
    fn numbers_read_like_python() {
        assert_eq!(python_number(85.0), "85");
        assert_eq!(python_number(0.126), "0.126");
        assert_eq!(python_number(-2.5), "-2.5");
        assert_eq!(python_number(1.5e-5), "1.5e-05");
        assert_eq!(python_number(-0.0), "0");
        assert_eq!(python_number(1e16), "10000000000000000");
    }
}
