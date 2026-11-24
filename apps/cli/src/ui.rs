//! Small interactive prompts. When not run from a terminal (scripts, tests)
//! the defaults are used and nothing is asked.

use anyhow::Result;
use dialoguer::{theme::ColorfulTheme, Input, Select};
use std::io::IsTerminal;

use crate::quality::Quality;

pub fn interactive() -> bool {
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

pub fn select(prompt: &str, items: &[String], default: usize) -> Result<usize> {
    Ok(Select::with_theme(&ColorfulTheme::default()).with_prompt(prompt).items(items).default(default).interact()?)
}

pub fn input(prompt: &str) -> Result<String> {
    Ok(Input::<String>::with_theme(&ColorfulTheme::default()).with_prompt(prompt).interact_text()?)
}

pub fn choose_quality() -> Result<Quality> {
    if !interactive() {
        return Ok(Quality::Balanced);
    }
    println!("\nQuality: there is a trade-off between delay and sharpness.");
    for q in Quality::ALL {
        println!("  * {}\n      {}", q.title(), q.explain());
    }
    println!();
    let labels: Vec<String> = Quality::ALL.iter().map(|q| q.title().to_string()).collect();
    let i = select("Choose quality", &labels, 1)?;
    Ok(Quality::ALL[i])
}
