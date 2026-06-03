use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::PathBuf;
use std::time::Instant;

use clap::{Parser, Subcommand};
use coldrun_core::exec::{execute, QueryResult};
use coldrun_core::storage::{load_demo_hits, load_parquet_into_table};
use coldrun_core::Database;

#[derive(Parser)]
#[command(name = "coldrun", about = "A smol columnar SQL toy")]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Data directory (default: .coldrun)
    #[arg(long, global = true, default_value = ".coldrun")]
    data_dir: PathBuf,
}

#[derive(Subcommand)]
enum Commands {
    /// Embedded mode: run SQL without a daemon
    Local {
        /// SQL string (otherwise read stdin)
        #[arg(short, long)]
        sql: Option<String>,

        /// Load hits.parquet then exit
        #[arg(long)]
        load: Option<PathBuf>,

        /// Load synthetic demo data (for dev only)
        #[arg(long)]
        demo: Option<u64>,
    },
    /// Start SQL server
    Serve {
        #[arg(long, default_value = "127.0.0.1:9000")]
        listen: String,
    },
    /// Interactive / batch client
    Client {
        #[arg(long, default_value = "127.0.0.1:9000")]
        host: String,
        /// SQL string (otherwise read stdin)
        #[arg(short, long)]
        sql: Option<String>,
    },
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "coldrun=info".into()),
        )
        .init();

    let cli = Cli::parse();
    match cli.command {
        Commands::Local { sql, load, demo } => {
            let mut db = Database::open(&cli.data_dir)?;
            if let Some(rows) = demo {
                let n = load_demo_hits(&mut db, rows)?;
                eprintln!("loaded demo {n} rows into hits");
                if sql.is_none() && load.is_none() {
                    return Ok(());
                }
            }
            if let Some(path) = load {
                let rows = load_parquet_into_table(&mut db, "hits", path)?;
                eprintln!("loaded {rows} rows into hits");
                if sql.is_none() {
                    return Ok(());
                }
            }
            let sql = read_sql(sql)?;
            run_timed(&mut db, &sql)?;
        }
        Commands::Serve { listen } => {
            serve(&cli.data_dir, &listen)?;
        }
        Commands::Client { host, sql } => {
            let sql = read_sql(sql)?;
            client_query(&host, &sql)?;
        }
    }
    Ok(())
}

fn read_sql(sql: Option<String>) -> anyhow::Result<String> {
    Ok(match sql {
        Some(s) => s,
        None => {
            let mut buf = String::new();
            io::stdin().read_to_string(&mut buf)?;
            buf
        }
    })
}

fn run_timed(db: &mut Database, sql: &str) -> anyhow::Result<()> {
    let start = Instant::now();
    let result = execute(db, sql.trim())?;
    let elapsed = start.elapsed();
    print_result(&result);
    eprintln!("{:.3}", elapsed.as_secs_f64());
    Ok(())
}

fn print_result(result: &QueryResult) {
    if !result.columns.is_empty() {
        println!("{}", result.columns.join("\t"));
    }
    for row in &result.rows {
        println!("{}", row.join("\t"));
    }
}

fn serve(data_dir: &PathBuf, listen: &str) -> anyhow::Result<()> {
    let mut db = Database::open(data_dir)?;
    let listener = TcpListener::bind(listen)?;
    eprintln!("coldrun listening on {listen} (data_dir={})", data_dir.display());
    for stream in listener.incoming() {
        let mut stream = stream?;
        if let Err(e) = handle_connection(&mut db, &mut stream) {
            let _ = writeln!(stream, "ERROR: {e}");
        }
    }
    Ok(())
}

fn handle_connection(db: &mut Database, stream: &mut TcpStream) -> anyhow::Result<()> {
    let mut buf = String::new();
    stream.read_to_string(&mut buf)?;
    let sql = buf.trim();
    if sql.is_empty() {
        return Ok(());
    }
    let start = Instant::now();
    let result = execute(db, sql)?;
    let elapsed = start.elapsed();
    print_result_to(stream, &result)?;
    writeln!(stream, "-- {:.3}s", elapsed.as_secs_f64())?;
    let _ = stream.shutdown(Shutdown::Write);
    Ok(())
}

fn print_result_to(w: &mut impl Write, result: &QueryResult) -> io::Result<()> {
    if !result.columns.is_empty() {
        writeln!(w, "{}", result.columns.join("\t"))?;
    }
    for row in &result.rows {
        writeln!(w, "{}", row.join("\t"))?;
    }
    Ok(())
}

fn client_query(host: &str, sql: &str) -> anyhow::Result<()> {
    let bench_mode = std::env::var_os("COLDRUN_BENCH").is_some();
    let mut stream = TcpStream::connect(host)?;
    stream.write_all(sql.as_bytes())?;
    if !sql.ends_with('\n') {
        stream.write_all(b"\n")?;
    }
    stream.shutdown(Shutdown::Write)?;
    let mut reader = BufReader::new(stream);
    let mut timing: Option<f64> = None;
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            break;
        }
        if let Some(t) = parse_timing_footer(line.trim_end()) {
            timing = Some(t);
            continue;
        }
        if !bench_mode {
            print!("{line}");
        }
    }
    if let Some(t) = timing {
        eprintln!("{t:.3}");
    }
    Ok(())
}

/// Server footer: `-- 0.123s` (ClickBench timing goes on stderr via `query`).
fn parse_timing_footer(line: &str) -> Option<f64> {
    let rest = line.strip_prefix("-- ")?;
    let num = rest.strip_suffix('s')?;
    num.parse().ok()
}
