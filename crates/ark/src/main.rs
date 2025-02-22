//
// main.rs
//
// Copyright (C) 2022-2024 Posit Software, PBC. All rights reserved.
//
//

#![allow(unused_unsafe)]

use std::cell::Cell;
use std::env;
use std::sync::Arc;
use std::sync::Mutex;

use amalthea::connection_file::ConnectionFile;
use amalthea::kernel::Kernel;
use amalthea::kernel_spec::KernelSpec;
use amalthea::socket::stdin::StdInRequest;
use ark::control::Control;
use ark::dap;
use ark::interface::SessionMode;
use ark::logger;
use ark::lsp;
use ark::request::KernelRequest;
use ark::request::RRequest;
use ark::shell::Shell;
use ark::signals::initialize_signal_block;
use ark::traps::register_trap_handlers;
use ark::version::detect_r;
use bus::Bus;
use crossbeam::channel::bounded;
use crossbeam::channel::unbounded;
use log::*;
use notify::Watcher;
use stdext::unwrap;

thread_local! {
    pub static ON_R_THREAD: Cell<bool> = Cell::new(false);
}

fn start_kernel(
    connection_file: ConnectionFile,
    r_args: Vec<String>,
    startup_file: Option<String>,
    session_mode: SessionMode,
    capture_streams: bool,
) {
    // Create a new kernel from the connection file
    let mut kernel = match Kernel::new("ark", connection_file) {
        Ok(k) => k,
        Err(e) => {
            error!("Failed to create kernel: {}", e);
            return;
        },
    };

    // Create the channels used for communication. These are created here
    // as they need to be shared across different components / threads.
    let iopub_tx = kernel.create_iopub_tx();

    // A broadcast channel (bus) used to notify clients when the kernel
    // has finished initialization.
    let mut kernel_init_tx = Bus::new(1);

    // A channel pair used for shell requests.
    // These events are used to manage the runtime state, and also to
    // handle message delivery, among other things.
    let (r_request_tx, r_request_rx) = bounded::<RRequest>(1);
    let (kernel_request_tx, kernel_request_rx) = bounded::<KernelRequest>(1);

    // Create the LSP and DAP clients.
    // Not all Amalthea kernels provide these, but ark does.
    // They must be able to deliver messages to the shell channel directly.
    let lsp = Arc::new(Mutex::new(lsp::handler::Lsp::new(kernel_init_tx.add_rx())));

    // DAP needs the `RRequest` channel to communicate with
    // `read_console()` and send commands to the debug interpreter
    let dap = dap::Dap::new_shared(r_request_tx.clone());

    // Communication channel between the R main thread and the Amalthea
    // StdIn socket thread
    let (stdin_request_tx, stdin_request_rx) = bounded::<StdInRequest>(1);

    // Communication channel for `CommEvent`
    let comm_manager_tx = kernel.create_comm_manager_tx();

    // Create the shell.
    let kernel_init_rx = kernel_init_tx.add_rx();
    let shell = Shell::new(
        comm_manager_tx.clone(),
        iopub_tx.clone(),
        r_request_tx.clone(),
        stdin_request_tx.clone(),
        kernel_init_rx,
        kernel_request_tx,
        kernel_request_rx,
    );

    // Create the control handler; this is used to handle shutdown/interrupt and
    // related requests
    let control = Arc::new(Mutex::new(Control::new(r_request_tx.clone())));

    // Create the stream behavior; this determines whether the kernel should
    // capture stdout/stderr and send them to the frontend as IOPub messages
    let stream_behavior = match capture_streams {
        true => amalthea::kernel::StreamBehavior::Capture,
        false => amalthea::kernel::StreamBehavior::None,
    };

    // Create the kernel
    let kernel_clone = shell.kernel.clone();
    let shell = Arc::new(Mutex::new(shell));

    let (stdin_reply_tx, stdin_reply_rx) = unbounded();

    let res = kernel.connect(
        shell,
        control,
        Some(lsp),
        Some(dap.clone()),
        stream_behavior,
        stdin_request_rx,
        stdin_reply_tx,
    );
    if let Err(err) = res {
        panic!("Couldn't connect to frontend: {err:?}");
    }

    // Start the R REPL (does not return for the duration of the session)
    ark::interface::start_r(
        r_args,
        startup_file,
        kernel_clone,
        comm_manager_tx,
        r_request_rx,
        stdin_request_tx,
        stdin_reply_rx,
        iopub_tx,
        kernel_init_tx,
        dap,
        session_mode,
    )
}

// Installs the kernelspec JSON file into one of Jupyter's search paths.
fn install_kernel_spec() {
    // Create the environment set for the kernel spec
    let mut env = serde_json::Map::new();

    // Detect the active version of R and set the R_HOME environment variable
    // accordingly
    let r_version = detect_r().unwrap();
    env.insert(
        "R_HOME".to_string(),
        serde_json::Value::String(r_version.r_home.clone()),
    );

    // Point `LD_LIBRARY_PATH` to a folder with some `libR.so`. It doesn't
    // matter which one, but the linker needs to be able to find a file of that
    // name, even though we won't use it for symbol resolution.
    // https://github.com/posit-dev/positron/issues/1619#issuecomment-1971552522
    if cfg!(target_os = "linux") {
        let lib = format!("{}/lib", r_version.r_home.clone());
        env.insert("LD_LIBRARY_PATH".into(), serde_json::Value::String(lib));
    }

    // Create the kernelspec
    let exe_path = unwrap!(env::current_exe(), Err(error) => {
        eprintln!("Failed to determine path to Ark. {}", error);
        return;
    });

    let spec = KernelSpec {
        argv: vec![
            String::from(exe_path.to_string_lossy()),
            String::from("--connection_file"),
            String::from("{connection_file}"),
            String::from("--session-mode"),
            String::from("notebook"),
        ],
        language: String::from("R"),
        display_name: String::from("Ark R Kernel"),
        env,
    };

    let dest = unwrap!(spec.install(String::from("ark")), Err(error) => {
        eprintln!("Failed to install Ark's Jupyter kernelspec. {}", error);
        return;
    });

    println!(
        "Successfully installed Ark Jupyter kernelspec.

    R ({}.{}.{}): {}
    Kernel: {}
    ",
        r_version.major,
        r_version.minor,
        r_version.patch,
        r_version.r_home,
        dest.to_string_lossy()
    );
}

fn parse_file(
    connection_file: &String,
    r_args: Vec<String>,
    startup_file: Option<String>,
    session_mode: SessionMode,
    capture_streams: bool,
) {
    match ConnectionFile::from_file(connection_file) {
        Ok(connection) => {
            info!(
                "Loaded connection information from frontend in {}",
                connection_file
            );
            debug!("Connection data: {:?}", connection);
            start_kernel(
                connection,
                r_args,
                startup_file,
                session_mode,
                capture_streams,
            );
        },
        Err(error) => {
            error!(
                "Couldn't read connection file {}: {:?}",
                connection_file, error
            );
        },
    }
}

fn print_usage() {
    println!("Ark {}, an R Kernel.", env!("CARGO_PKG_VERSION"));
    println!(
        r#"
Usage: ark [OPTIONS]

Available options:

--connection_file FILE   Start the kernel with the given JSON connection file
                         (see the Jupyter kernel documentation for details)
-- arg1 arg2 ...         Set the argument list to pass to R; defaults to
                         --interactive
--startup-file FILE      An R file to run on session startup
--session-mode MODE      The mode in which the session is running (console, notebook, background)
--no-capture-streams     Do not capture stdout/stderr from R
--version                Print the version of Ark
--log FILE               Log to the given file (if not specified, stdout/stderr
                         will be used)
--install                Install the kernel spec for Ark
--help                   Print this help message
"#
    );
}

fn main() {
    ON_R_THREAD.set(true);

    // Block signals in this thread (and any child threads).
    initialize_signal_block();

    // Get an iterator over all the command-line arguments
    let mut argv = std::env::args();

    // Skip the first "argument" as it's the path/name to this executable
    argv.next();

    let mut connection_file: Option<String> = None;
    let mut startup_file: Option<String> = None;
    let mut session_mode = SessionMode::Console;
    let mut log_file: Option<String> = None;
    let mut profile_file: Option<String> = None;
    let mut startup_notifier_file: Option<String> = None;
    let mut startup_delay: Option<std::time::Duration> = None;
    let mut r_args: Vec<String> = Vec::new();
    let mut has_action = false;
    let mut capture_streams = true;

    // Process remaining arguments. TODO: Need an argument that can passthrough args to R
    while let Some(arg) = argv.next() {
        match arg.as_str() {
            "--connection_file" => {
                if let Some(file) = argv.next() {
                    connection_file = Some(file);
                    has_action = true;
                } else {
                    eprintln!(
                        "A connection file must be specified with the --connection_file argument."
                    );
                    break;
                }
            },
            "--startup-file" => {
                if let Some(file) = argv.next() {
                    startup_file = Some(file);
                    has_action = true;
                } else {
                    eprintln!("A startup file must be specified with the --startup-file argument.");
                    break;
                }
            },
            "--session-mode" => {
                if let Some(mode) = argv.next() {
                    session_mode = match mode.as_str() {
                        "console" => SessionMode::Console,
                        "notebook" => SessionMode::Notebook,
                        "background" => SessionMode::Background,
                        _ => {
                            eprintln!("Invalid session mode: '{}' (expected console, notebook, or background)", mode);
                            break;
                        },
                    };
                } else {
                    eprintln!("A session mode must be specified with the --session-mode argument.");
                    break;
                }
            },
            "--version" => {
                println!("Ark {}", env!("CARGO_PKG_VERSION"));
                has_action = true;
            },
            "--install" => {
                install_kernel_spec();
                has_action = true;
            },
            "--help" => {
                print_usage();
                has_action = true;
            },
            "--no-capture-streams" => capture_streams = false,
            "--log" => {
                if let Some(file) = argv.next() {
                    log_file = Some(file);
                } else {
                    eprintln!("A log file must be specified with the --log argument.");
                    break;
                }
            },
            "--profile" => {
                if let Some(file) = argv.next() {
                    profile_file = Some(file);
                } else {
                    eprintln!("A profile file must be specified with the --profile argument.");
                    break;
                }
            },
            "--startup-notifier-file" => {
                if let Some(file) = argv.next() {
                    startup_notifier_file = Some(file);
                } else {
                    eprintln!(
                        "A notification file must be specified with the --startup-notifier-file argument."
                    );
                    break;
                }
            },
            "--startup-delay" => {
                if let Some(delay_arg) = argv.next() {
                    if let Ok(delay) = delay_arg.parse::<u64>() {
                        startup_delay = Some(std::time::Duration::from_secs(delay));
                    } else {
                        eprintln!("Can't parse delay in seconds");
                        break;
                    }
                } else {
                    eprintln!(
                        "A delay in seconds must be specified with the --startup-delay argument."
                    );
                    break;
                }
            },
            "--" => {
                // Consume the rest of the arguments for passthrough delivery to R
                while let Some(arg) = argv.next() {
                    r_args.push(arg);
                }
                break;
            },
            other => {
                eprintln!("Argument '{}' unknown", other);
                break;
            },
        }
    }

    // Initialize the logger.
    logger::init(log_file.as_deref(), profile_file.as_deref());

    if let Some(file) = startup_notifier_file {
        let path = std::path::Path::new(&file);
        let (tx, rx) = unbounded();

        if let Err(err) = (|| -> anyhow::Result<()> {
            let config = notify::Config::default()
                .with_poll_interval(std::time::Duration::from_millis(2))
                .with_compare_contents(false);

            let handler = move |x| {
                let _ = tx.send(x);
            };
            let mut watcher = notify::RecommendedWatcher::new(handler, config).unwrap();
            watcher.watch(path, notify::RecursiveMode::NonRecursive)?;

            loop {
                let ev = rx.recv()?;
                match ev.unwrap().kind {
                    notify::event::EventKind::Modify(_) => {
                        break;
                    },
                    notify::event::EventKind::Remove(_) => {
                        break;
                    },
                    _ => {
                        continue;
                    },
                }
            }

            watcher.unwatch(path)?;
            Ok(())
        })() {
            eprintln!("Problem with the delay file: {:?}", err);
        }
    }

    if let Some(delay) = startup_delay {
        std::thread::sleep(delay);
    }

    // If the user didn't specify an action, print the usage instructions and
    // exit
    if !has_action {
        print_usage();
        return;
    }

    // Register segfault handler to get a backtrace. Should be after
    // initialising `log!`. Note that R will not override this handler
    // because we set `R_SignalHandlers` to 0 before startup.
    register_trap_handlers();

    // If the r_args vector is empty, add `--interactive` to the list of
    // arguments to pass to R.
    if r_args.is_empty() {
        r_args.push(String::from("--interactive"));
    }

    // This causes panics on background threads to propagate on the main
    // thread. If we don't propagate a background thread panic, the program
    // keeps running in an unstable state as all communications with this
    // thread will error out or panic.
    // https://stackoverflow.com/questions/35988775/how-can-i-cause-a-panic-on-a-thread-to-immediately-end-the-main-thread
    let old_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let info = panic_info.payload();

        let loc = if let Some(location) = panic_info.location() {
            format!("In file '{}' at line {}:", location.file(), location.line(),)
        } else {
            String::from("No location information:")
        };

        let append_trace = |info: &str| -> String {
            // Top-level-exec and try-catch errors already contain a backtrace
            // for the R thread so don't repeat it if we see one. Only perform
            // this check on the R thread because we do want other threads'
            // backtraces if the panic occurred elsewhere.
            if ON_R_THREAD.get() && info.contains("\n{R_BACKTRACE_HEADER}\n") {
                String::from("")
            } else {
                format!(
                    "\n\nBacktrace:\n{}",
                    std::backtrace::Backtrace::force_capture()
                )
            }
        };

        // Report panic to the frontend
        if let Some(info) = info.downcast_ref::<&str>() {
            let trace = append_trace(info);
            log::error!("Panic! {loc} {info:}{trace}");
        } else if let Some(info) = info.downcast_ref::<String>() {
            let trace = append_trace(&info);
            log::error!("Panic! {loc} {info:}{trace}");
        } else {
            let trace = format!("Backtrace:\n{}", std::backtrace::Backtrace::force_capture());
            log::error!("Panic! {loc} No contextual information.\n{trace}");
        }

        // Give some time to flush log
        log::logger().flush();
        std::thread::sleep(std::time::Duration::from_millis(250));

        old_hook(panic_info);
        std::process::abort();
    }));

    // Parse the connection file and start the kernel
    if let Some(connection) = connection_file {
        parse_file(
            &connection,
            r_args,
            startup_file,
            session_mode,
            capture_streams,
        );
    }
}
