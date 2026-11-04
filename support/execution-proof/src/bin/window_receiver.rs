// SPDX-License-Identifier: MPL-2.0
#![cfg_attr(windows, windows_subsystem = "windows")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    cue_execution_proof::receiver::run()
}
