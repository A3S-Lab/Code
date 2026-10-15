//! Convert a recording of the real pager to lossless terminal cell data.
use std::error::Error;
use std::fmt::Write;
use std::path::Path;

fn quoted(value: &str) -> String {
    let mut out = String::from("\"");
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => { let _ = write!(out, "\\u{:04x}", c as u32); }
            c => out.push(c),
        }
    }
    out.push('"'); out
}

fn color(c: vt100::Color) -> String {
    match c {
        vt100::Color::Default => "null".into(),
        vt100::Color::Idx(n) => n.to_string(),
        vt100::Color::Rgb(r, g, b) => format!("\"#{r:02x}{g:02x}{b:02x}\""),
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 { return Err("usage: vt_cells recording.vt cells.json".into()); }
    let raw = std::fs::read(&args[1])?;
    let mut parser = vt100::Parser::new(35, 110, 2000);
    parser.process(&raw);
    let screen = parser.screen();
    let (rows, cols) = screen.size();
    let (cy, cx) = screen.cursor_position();
    let mut json = format!("{{\"rows\":{rows},\"cols\":{cols},\"cursor\":[{cx},{cy},{}],\"text\":{},\"cells\":[", !screen.hide_cursor(), quoted(&screen.contents()));
    let mut first = true;
    for y in 0..rows {
        for x in 0..cols {
            if let Some(cell) = screen.cell(y, x) {
                if !first { json.push(','); } first = false;
                write!(json, "[{x},{y},{},{},{},{},{},{},{},{}]",
                    quoted(&cell.contents()), color(cell.fgcolor()), color(cell.bgcolor()),
                    cell.bold(), cell.italic(), cell.underline(), cell.inverse(), cell.is_wide_continuation())?;
            }
        }
    }
    json.push_str("]}\n");
    std::fs::write(Path::new(&args[2]), json)?;
    Ok(())
}
