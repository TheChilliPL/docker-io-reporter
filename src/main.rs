use std::fs::OpenOptions;
use std::io::{stdout, Write};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use tokio::fs::{read_link, read_to_string, remove_file, rename, File};
use eyre::{eyre, ContextCompat, OptionExt, Result, WrapErr};
use bollard::Docker;
use bollard::models::ContainerSummary;
use bollard::query_parameters::{InspectContainerOptions, ListContainersOptions};
use clap::{Parser, Subcommand};
use hyper::body::Incoming;
use hyper::{Request, Response};
use hyper::server::conn::http1;
use hyper_util::rt::TokioIo;
use log::{debug, error, info, trace, LevelFilter};
use tokio::net::TcpListener;

fn get_container_name(container: &ContainerSummary) -> Result<&str> {
    let names = container.names.as_ref().ok_or_eyre("Container has no name")?;
    let full_name = names.first().ok_or_eyre("Container has no name")?;
    if full_name.starts_with("/") {
        Ok(&full_name[1..])
    } else {
        Ok(&full_name[..])
    }
}

fn write_utf8(output: &mut dyn Write, string: &str) -> std::io::Result<()> {
    output.write(string.as_bytes())?;
    Ok(())
}

async fn get_device_name(device_maj_min: &str) -> Result<String> {
    let sys_path = Path::new("/sys/dev/block").join(device_maj_min);

    let target_path = read_link(sys_path).await?;

    Ok(target_path.file_name()
        .wrap_err("Couldn't get filename of device path")?
        .to_str()
        .wrap_err("Couldn't convert device name to string")?
        .to_owned())
}

async fn process_iostat(container_name: &str, iostat_output: &str, output: &mut dyn Write) -> Result<()> {
    for line in iostat_output.lines() {
        let mut entries = line.split_ascii_whitespace();

        let device_name = get_device_name(&entries.next().ok_or_eyre("Couldn't get device ID")?).await?;

        for entry in entries {
            let (key, value) = entry.split_once('=').ok_or_eyre("Failed to split entry")?;

            write_utf8(output, &format!("docker_iostat_{key}{{device=\"{device_name}\",container=\"{container_name}\"}} {value}\n"))?;
        }
    }

    Ok(())
}

async fn process_iopressure(container_name: &str, iopressure_output: &str, output: &mut dyn Write) -> Result<()> {
    for line in iopressure_output.lines() {
        let mut entries = line.split_ascii_whitespace();

        let entry_type = entries.next().ok_or_eyre("Couldn't get entry type")?;

        for entry in entries {
            let (key, value) = entry.split_once('=').ok_or_eyre("Failed to split entry")?;

            write_utf8(output, &format!("docker_iopressure_{key}{{type=\"{entry_type}\",container=\"{container_name}\"}} {value}\n"))?;
        }
    }

    Ok(())
}

async fn process_container(docker: &Docker, name: &str, output: &mut dyn Write) -> Result<()> {
    let inspected = docker.inspect_container(name, None::<InspectContainerOptions>).await?;

    let state = inspected.state.ok_or_eyre("Error reading container state")?;

    let pid = state.pid.ok_or_eyre("Error reading container pid")?;

    trace!("Container state PID: {}", pid);

    let cgroup_output = read_to_string(format!("/proc/{}/cgroup", pid)).await
        .wrap_err("Error reading cgroup information")?;

    if !cgroup_output.starts_with("0::/") {
        return Err(eyre!("Error parsing cgroup. Are you sure you're on cgroup v2?"));
    }

    let cgroup = cgroup_output[4..].trim();

    trace!("Cgroup output: {}", cgroup);

    let cgroup_path = Path::new("/sys/fs/cgroup").join(cgroup);

    trace!("Full cgroup path: {}", cgroup_path.display());

    let iostat_path = cgroup_path.join("io.stat");
    let iopressure_path = cgroup_path.join("io.pressure");

    trace!("IOstat path: {}", iostat_path.display());

    let iostat = read_to_string(iostat_path).await?;
    let iopressure = read_to_string(iopressure_path).await?;

    process_iostat(name, &iostat, output).await?;

    process_iopressure(name, &iopressure, output).await?;

    Ok(())
}

/// Program that uses cgroup v2 to report container IO statistics in a Prometheus format
#[derive(Parser, Debug)]
#[command(version, about)]
struct Cli {
    #[clap(subcommand)]
    subcommand: CliSubcommand,
}

#[derive(Subcommand, Debug)]
enum CliSubcommand {
    /// Hosts a Prometheus-compatible web server.
    Host {
        /// IP to which the server will bind.
        ///
        /// 0.0.0.0 means any IPv4 address.
        #[arg(default_value = "0.0.0.0", env = "DOCKER_IO_REPORTER_IP")]
        ip: IpAddr,
        /// Port on which to start the server.
        #[arg(short, long, default_value_t = 9100, env = "DOCKER_IO_REPORTER_PORT")]
        port: u16,
    },
    /// Saves current stats to the file or standard output.
    Save {
        /// Path of file to which the output will be saved.
        ///
        /// By default, outputs to standard output.
        #[arg()]
        path: Option<PathBuf>,
        /// Does file save atomically, by first writing to a temp file and then performing an atomic rename.
        ///
        /// Requires path to be set.
        #[arg(short, long, requires = "path")]
        atomic: bool,
    },
}

async fn save_stats(output: &mut dyn Write) -> Result<()> {
    let docker = Docker::connect_with_defaults()?;

    let containers = docker.list_containers(None::<ListContainersOptions>).await?;

    for container in containers {
        let name = match get_container_name(&container) {
            Ok(name) => name,
            Err(_) => continue,
        };

        trace!("Container with name: {}", name);

        let result = process_container(&docker, &name, output).await;

        if let Err(e) = result {
            error!("Error processing container: {:?}", e);
        }
    }

    output.flush()?;

    Ok(())
}

async fn handle_request(req: Request<Incoming>) -> Result<Response<String>, hyper::Error> {
    debug!("Request received: {:?}", req);

    let mut buffer = Vec::with_capacity(2048);

    save_stats(&mut buffer).await.unwrap();

    // SAFETY: save_stats only writes valid strings to buffer
    let stats_str = unsafe { String::from_utf8_unchecked(buffer) };

    Ok(Response::builder()
        .header("Content-Type", "text/plain")
        .body(stats_str)
        .unwrap())
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    pretty_env_logger::formatted_timed_builder()
        .filter_level(LevelFilter::Info)
        .filter_module("docker_io_reporter", LevelFilter::Debug)
        .parse_default_env()
        .init();

    match cli.subcommand {
        CliSubcommand::Host { ip, port } => {
            let listener = TcpListener::bind((ip, port)).await?;

            info!("Listening at http://{}:{}", ip, port);

            loop {
                let (socket, addr) = listener.accept().await?;

                let io = TokioIo::new(socket);
                let service = hyper::service::service_fn(handle_request);

                let result = http1::Builder::new().serve_connection(io, service).await;

                if let Err(err) = result {
                    error!("Service failed: {}", err);
                }
            }
        }
        CliSubcommand::Save { path, atomic } => {
            let output: &mut dyn Write = match path.as_ref() {
                Some(path) => {
                    let path = if atomic {
                        path.with_file_name(format!("{}.atomic", path.file_name().unwrap().to_str().unwrap()))
                    } else { path.to_owned() };

                    &mut OpenOptions::new()
                        .write(true)
                        .create(true)
                        .truncate(true)
                        .open(path)?
                }
                None => &mut stdout(),
            };

            save_stats(output).await?;

            if atomic {
                let path = path.unwrap();
                // if path.try_exists()? {
                //     remove_file(path).await?;
                // }

                let old_path = path.with_file_name(format!("{}.atomic", path.file_name().unwrap().to_str().unwrap()));

                rename(old_path, path).await?;
            }
        }
    }

    Ok(())
}
