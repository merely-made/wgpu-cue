// SPDX-License-Identifier: MPL-2.0
//! Shared exact receiving-window input for the CPU attribution probe.
pub fn html(font: &str) -> String {
    let rows=(0..120).map(|i|format!("<section style='background:#{:02x}4060;padding:12px;margin:6px;border:2px solid #90b0d0'><b>Record {i}</b> — owned document, native frame transfer, glyphs and borders. <span>Scroll this retained document.</span></section>",32+i%80)).collect::<String>();
    format!(
        "<!doctype html><style>body{{margin:0;background:#203050;color:white;font:16px {}}}h1{{margin:12px}}</style><h1>Genet renderer process → receiving window</h1>{rows}",
        font
    )
}
