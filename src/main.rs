mod cli;
mod demo;
mod discovery;
mod doctor;
mod edid;
mod export;
mod hyprland;
mod hyprland_config;
mod install;
mod models;
mod timings;
mod tui;
mod validation;
mod workspace;

use anyhow::Result;

fn main() -> Result<()> {
    match cli::parse_env_args() {
        Ok(cli::Command::Tui) => {
            let monitors = discovery::discover_monitors()?;
            tui::App::new(monitors).run()
        }
        Ok(cli::Command::Demo) => tui::App::new(demo::monitors()?).run(),
        Ok(cli::Command::Doctor) => doctor::run(),
        Ok(cli::Command::Help) => {
            println!("{}", cli::help_text());
            Ok(())
        }
        Ok(cli::Command::Version) => {
            println!("drmcru {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Err(error) => {
            eprintln!("{error}\n\n{}", cli::help_text());
            std::process::exit(2);
        }
    }
}
