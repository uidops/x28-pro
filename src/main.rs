//! x28-pro — print the CPU temperature from System Status > Device Information.
//!
//! The menu bar version of this tool lives in `src/bin/x28-tray/`.

use std::env;
use std::process::ExitCode;
use std::time::Instant;

use x28_pro::{Client, Config};

struct Opts {
    host: Option<String>,
    user: Option<String>,
    pass: Option<String>,
    show_time: bool,
    details: bool,
    capture_ui: bool,
}

fn usage() -> String {
    format!(
        "x28-pro — CPU temperature from ZLT X28 PRO (System Status > Device Information)

USAGE:
    x28-pro [OPTIONS]

OPTIONS:
    --host <URL>   Router base URL   [default: https://192.168.1.1]  (env: X28_HOST)
    --user <NAME>  Login user        [default: superadmin]          (env: X28_USER)
    --pass <PASS>  Login password    [default: superadmin]          (env: X28_PASS)
    --details      Also print device, network, band and RF details
    --capture-ui   Save the router's web UI and an index of its API commands
                   to {} (for mapping more settings)
    --time         Print elapsed time to stderr
    -h, --help     Show this help

Config is also read from/written to {}",
        Config::web_ui_dir().display(),
        Config::path().display()
    )
}

fn bail(msg: impl AsRef<str>) -> ! {
    eprintln!("error: {}", msg.as_ref());
    eprint!("{}", usage());
    std::process::exit(2);
}

fn parse_args() -> Opts {
    let mut opts = Opts {
        host: None,
        user: None,
        pass: None,
        show_time: false,
        details: false,
        capture_ui: false,
    };

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut take_value = |name: &str| -> String {
            match args.next() {
                Some(v) => v,
                None => bail(format!("{name} requires a value")),
            }
        };
        match arg.as_str() {
            "--host" => opts.host = Some(take_value("--host")),
            "--user" => opts.user = Some(take_value("--user")),
            "--pass" => opts.pass = Some(take_value("--pass")),
            "--time" => opts.show_time = true,
            "--details" => opts.details = true,
            "--capture-ui" => opts.capture_ui = true,
            "-h" | "--help" => {
                print!("{}", usage());
                std::process::exit(0);
            }
            other => bail(format!("unknown argument {other:?}")),
        }
    }
    opts
}

fn main() -> ExitCode {
    let opts = parse_args();
    let started = Instant::now();

    let mut config = Config::load();
    if let Some(h) = opts.host {
        config.host = h;
    }
    if let Some(u) = opts.user {
        config.user = u;
    }
    if let Some(p) = opts.pass {
        config.pass = p;
    }
    config.normalize();

    if opts.capture_ui {
        let dir = Config::web_ui_dir();
        return match Client::new(&config).capture_web_ui(&dir) {
            Ok(c) => {
                println!(
                    "Saved {} files with {} API commands to {}",
                    c.files,
                    c.commands,
                    dir.display()
                );
                ExitCode::SUCCESS
            }
            Err(err) => {
                eprintln!("error: {err}");
                ExitCode::FAILURE
            }
        };
    }

    let result = Client::new(&config).fetch();
    if opts.show_time {
        eprintln!("elapsed: {:.1} ms", started.elapsed().as_secs_f64() * 1000.0);
    }

    match result {
        Ok(status) => {
            println!("CPU temperature: {}", status.temperature_label());
            if opts.details {
                println!("Device: {}", status.board.as_deref().unwrap_or("—"));
                println!("Network: {}", status.summary());
                println!("RF: {}", status.rf_line());
                if let Some(cell) = status.cell_id.as_ref() {
                    println!("Cell: {cell} · PCI {}", status.pci.as_deref().unwrap_or("—"));
                }
                println!("CPU load: {}%", status.cpu_load.as_deref().unwrap_or("—"));
                println!("Uptime: {}", status.uptime_label());
            }
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}
