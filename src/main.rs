use std::io::IsTerminal;
use std::time::Duration;

use ideas_tui::api::Client;
use ideas_tui::app::{App, Msg};
use ideas_tui::{config, ui};
use ratatui::crossterm::event::{self, Event};
use tokio::sync::mpsc::unbounded_channel;

#[tokio::main]
async fn main() {
    if std::env::args().any(|a| a == "--help" || a == "-h") {
        println!("ideas-tui: terminal client for the idea pipeline\n\n{}", config::SETUP_HELP);
        return;
    }
    let (cfg, warnings) = match config::load() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("ideas-tui: {e}");
            std::process::exit(1);
        }
    };
    for w in &warnings {
        eprintln!("warning: {w}");
    }
    if !std::io::stdout().is_terminal() {
        eprintln!("ideas-tui needs an interactive terminal");
        std::process::exit(1);
    }

    let (tx, mut rx) = unbounded_channel();
    let mut app = App::new(Client::new(&cfg), tx.clone());
    app.start();
    if let Some(w) = warnings.first() {
        app.error(w.clone());
    }

    // Terminal events and a ~10 fps tick (spinner, timers) on a plain thread.
    let input_tx = tx.clone();
    std::thread::spawn(move || {
        loop {
            let msg = match event::poll(Duration::from_millis(100)) {
                Ok(true) => match event::read() {
                    Ok(Event::Key(k)) => Msg::Key(k),
                    Ok(Event::Resize(..)) => Msg::Redraw,
                    Ok(_) => continue,
                    Err(_) => break,
                },
                Ok(false) => Msg::Tick,
                Err(_) => break,
            };
            if input_tx.send(msg).is_err() {
                break;
            }
        }
    });

    // `init` enters raw mode and the alternate screen, and installs a panic
    // hook that restores the terminal before the panic message prints.
    let mut terminal = ratatui::init();
    let result = run(&mut terminal, &mut app, &mut rx).await;
    ratatui::restore();
    if let Err(e) = result {
        eprintln!("ideas-tui: {e}");
        std::process::exit(1);
    }
}

async fn run(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<Msg>,
) -> std::io::Result<()> {
    terminal.draw(|f| ui::draw(f, app))?;
    while let Some(msg) = rx.recv().await {
        app.update(msg);
        // Fold in whatever else is already queued before drawing.
        while let Ok(more) = rx.try_recv() {
            app.update(more);
        }
        if app.should_quit {
            break;
        }
        terminal.draw(|f| ui::draw(f, app))?;
    }
    Ok(())
}
