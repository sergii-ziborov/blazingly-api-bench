//! Scenario load generator for the API comparison.
//!
//! Written for this repository rather than reused off the shelf because the
//! usual tools cannot measure what this suite needs. `go-wrk` reports every
//! sub-millisecond round trip as `0s`, which erases the percentile columns for
//! any of the Rust servers, and none of the common tools verify that two
//! servers actually returned equivalent responses before comparing their
//! throughput.
//!
//! Each connection keeps one request in flight, which is the ordinary
//! keep-alive shape, and records the round trip of every request.

use std::env;
use std::fmt::Write as _;
use std::io::{self, Read as _, Write as _};
use std::net::TcpStream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Clone)]
struct Config {
    address: String,
    method: String,
    path: String,
    body: Vec<u8>,
    headers: Vec<(String, String)>,
    connections: usize,
    duration: Duration,
    warmup: Duration,
    expect_status: u16,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::from_args()?;
    let request = Arc::<[u8]>::from(config.request_bytes());
    let barrier = Arc::new(Barrier::new(config.connections + 1));
    let requests = Arc::new(AtomicU64::new(0));
    let errors = Arc::new(AtomicU64::new(0));
    let status_mismatches = Arc::new(AtomicU64::new(0));
    let mut workers = Vec::with_capacity(config.connections);

    for _ in 0..config.connections {
        let config = config.clone();
        let request = Arc::clone(&request);
        let barrier = Arc::clone(&barrier);
        let requests = Arc::clone(&requests);
        let errors = Arc::clone(&errors);
        let mismatches = Arc::clone(&status_mismatches);
        workers.push(thread::spawn(move || {
            run_connection(&config, &request, &barrier, &requests, &errors, &mismatches)
        }));
    }

    barrier.wait();
    let started = Instant::now();
    let mut latencies_ns = Vec::new();
    for worker in workers {
        match worker.join() {
            Ok(samples) => latencies_ns.extend(samples),
            Err(_) => {
                errors.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    let elapsed = started.elapsed();
    let requests = requests.load(Ordering::Relaxed);
    let errors = errors.load(Ordering::Relaxed);
    let mismatches = status_mismatches.load(Ordering::Relaxed);

    println!("Connections:\t\t{}", config.connections);
    println!("Requests:\t\t{requests}");
    println!("Elapsed:\t\t{:.3}s", elapsed.as_secs_f64());
    println!(
        "Requests/sec:\t\t{:.2}",
        requests as f64 / elapsed.as_secs_f64()
    );
    println!("Number of Errors:\t{errors}");
    println!("Status mismatches:\t{mismatches}");
    print_latency_report(&mut latencies_ns);

    if errors == 0 && mismatches == 0 {
        Ok(())
    } else {
        Err("load run reported errors or unexpected statuses".into())
    }
}

fn print_latency_report(samples_ns: &mut Vec<u64>) {
    if samples_ns.is_empty() {
        println!("Latency samples:\t0");
        return;
    }
    samples_ns.sort_unstable();
    println!("Latency samples:\t{}", samples_ns.len());
    println!("Latency min:\t\t{}", format_ns(samples_ns[0]));
    for quantile in [0.50_f64, 0.75, 0.90, 0.95, 0.99, 0.999] {
        let value = percentile(samples_ns, quantile);
        let label = format_quantile(quantile);
        println!("Latency p{label:<7}\t{}", format_ns(value));
    }
    println!(
        "Latency max:\t\t{}",
        format_ns(samples_ns[samples_ns.len() - 1])
    );
    let sum: u128 = samples_ns.iter().map(|value| u128::from(*value)).sum();
    println!(
        "Latency mean:\t\t{}",
        format_ns((sum / samples_ns.len() as u128) as u64)
    );
}

fn percentile(sorted_ns: &[u64], quantile: f64) -> u64 {
    let last = sorted_ns.len() - 1;
    let rank = (quantile * last as f64).round() as usize;
    sorted_ns[rank.min(last)]
}

fn format_quantile(quantile: f64) -> String {
    let percent = quantile * 100.0;
    if (percent - percent.round()).abs() < f64::EPSILON {
        format!("{percent:.0}:")
    } else {
        format!("{percent:.1}:")
    }
}

fn format_ns(value: u64) -> String {
    if value < 1_000 {
        format!("{value}ns")
    } else if value < 1_000_000 {
        format!("{:.3}us", value as f64 / 1_000.0)
    } else {
        format!("{:.3}ms", value as f64 / 1_000_000.0)
    }
}

fn run_connection(
    config: &Config,
    request: &[u8],
    barrier: &Barrier,
    requests: &AtomicU64,
    errors: &AtomicU64,
    mismatches: &AtomicU64,
) -> Vec<u64> {
    let mut latencies_ns = Vec::new();
    let Ok(mut stream) = TcpStream::connect(&config.address) else {
        errors.fetch_add(1, Ordering::Relaxed);
        barrier.wait();
        return latencies_ns;
    };
    let timeout = config.duration + config.warmup + Duration::from_secs(5);
    if stream.set_nodelay(true).is_err()
        || stream.set_read_timeout(Some(timeout)).is_err()
        || stream.set_write_timeout(Some(timeout)).is_err()
    {
        errors.fetch_add(1, Ordering::Relaxed);
        barrier.wait();
        return latencies_ns;
    }

    let mut buffer = Vec::with_capacity(256 * 1024);
    let mut read_buffer = vec![0_u8; 256 * 1024];
    barrier.wait();

    // The warm-up requests are not recorded: they pay for lazy initialisation,
    // JIT-style first-call costs, and cold caches, none of which the steady
    // state contains.
    let warmup_deadline = Instant::now() + config.warmup;
    while Instant::now() < warmup_deadline {
        if exchange(&mut stream, request, &mut buffer, &mut read_buffer).is_err() {
            errors.fetch_add(1, Ordering::Relaxed);
            return latencies_ns;
        }
    }

    let deadline = Instant::now() + config.duration;
    while Instant::now() < deadline {
        let started = Instant::now();
        match exchange(&mut stream, request, &mut buffer, &mut read_buffer) {
            Ok(status) => {
                latencies_ns
                    .push(u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX));
                if status != config.expect_status {
                    mismatches.fetch_add(1, Ordering::Relaxed);
                }
                requests.fetch_add(1, Ordering::Relaxed);
            }
            Err(_) => {
                errors.fetch_add(1, Ordering::Relaxed);
                return latencies_ns;
            }
        }
    }
    latencies_ns
}

fn exchange(
    stream: &mut TcpStream,
    request: &[u8],
    buffer: &mut Vec<u8>,
    read_buffer: &mut [u8],
) -> io::Result<u16> {
    stream.write_all(request)?;
    buffer.clear();
    loop {
        if let Some((status, total)) = response_length(buffer)? {
            if buffer.len() >= total {
                return Ok(status);
            }
        }
        let read = stream.read(read_buffer)?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "server closed the connection",
            ));
        }
        buffer.extend_from_slice(&read_buffer[..read]);
    }
}

/// Returns the status and the total response length once the head is complete.
///
/// Only `Content-Length` framing is supported, which every scenario in the
/// spec uses. A chunked response makes the run fail loudly rather than being
/// mis-measured.
fn response_length(buffer: &[u8]) -> io::Result<Option<(u16, usize)>> {
    let mut headers = [httparse::EMPTY_HEADER; 48];
    let mut response = httparse::Response::new(&mut headers);
    let status = response
        .parse(buffer)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let httparse::Status::Complete(head_bytes) = status else {
        return Ok(None);
    };
    let code = response.code.unwrap_or(0);
    let mut content_length = None;
    for header in response.headers.iter() {
        if header.name.eq_ignore_ascii_case("transfer-encoding") {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "chunked responses are not supported by this scenario client",
            ));
        }
        if header.name.eq_ignore_ascii_case("content-length") {
            content_length = std::str::from_utf8(header.value)
                .ok()
                .and_then(|value| value.trim().parse::<usize>().ok());
        }
    }
    let length = content_length.ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "response has no Content-Length")
    })?;
    Ok(Some((code, head_bytes + length)))
}

impl Config {
    fn from_args() -> Result<Self, Box<dyn std::error::Error>> {
        let mut address = "127.0.0.1:3201".to_owned();
        let mut method = "GET".to_owned();
        let mut path = "/health".to_owned();
        let mut body_file: Option<String> = None;
        let mut headers = Vec::new();
        let mut connections = 64_usize;
        let mut duration = 10_u64;
        let mut warmup = 2_u64;
        let mut expect_status = 200_u16;

        let mut arguments = env::args().skip(1);
        while let Some(argument) = arguments.next() {
            let value = arguments
                .next()
                .ok_or_else(|| format!("missing value after {argument}"))?;
            match argument.as_str() {
                "--address" => address = value,
                "--method" => method = value.to_ascii_uppercase(),
                "--path" => path = value,
                "--body-file" => body_file = Some(value),
                "--header" => {
                    let (name, header_value) = value
                        .split_once(':')
                        .ok_or_else(|| format!("malformed header {value}"))?;
                    headers.push((
                        name.trim().to_ascii_lowercase(),
                        header_value.trim().to_owned(),
                    ));
                }
                "--connections" => connections = value.parse()?,
                "--duration" => duration = value.parse()?,
                "--warmup" => warmup = value.parse()?,
                "--expect-status" => expect_status = value.parse()?,
                _ => return Err(format!("unknown argument {argument}").into()),
            }
        }
        if connections == 0 || duration == 0 {
            return Err("connections and duration must be positive".into());
        }
        let body = match body_file {
            Some(path) => std::fs::read(path)?,
            None => Vec::new(),
        };
        Ok(Self {
            address,
            method,
            path,
            body,
            headers,
            connections,
            duration: Duration::from_secs(duration),
            warmup: Duration::from_secs(warmup),
            expect_status,
        })
    }

    fn request_bytes(&self) -> Vec<u8> {
        let mut head = String::with_capacity(256);
        let _ = write!(
            head,
            "{} {} HTTP/1.1\r\nhost: {}\r\nconnection: keep-alive\r\naccept: application/json\r\n",
            self.method, self.path, self.address
        );
        for (name, value) in &self.headers {
            let _ = write!(head, "{name}: {value}\r\n");
        }
        if !self.body.is_empty() {
            let _ = write!(head, "content-length: {}\r\n", self.body.len());
        }
        head.push_str("\r\n");
        let mut request = head.into_bytes();
        request.extend_from_slice(&self.body);
        request
    }
}
