use iris::config::load_config;
use iris::machine::Machine;

fn main() {
    print_build_features();

    let (cfg, scale) = load_config();
    let scroll_pixels_per_line = cfg.mouse_scroll_pixels_per_line;
    let lock_aspect_ratio = cfg.lock_aspect_ratio;
    let headless = cfg.headless;
    let gdb_port = cfg.gdb_port;
    let ci_enabled = cfg.ci;
    let ci_display = cfg.ci_display;
    let ci_socket_path = cfg.ci_socket.clone();

    // CI control socket will be started after Machine::new below (it needs a
    // pointer into the constructed Machine).

    // NFS is now served in-process by the NAT (src/nfsudp.rs) — no external
    // unfsd to spawn. The directory is created on demand by the server.

    // Machine::new() allocates >1MB on the stack (Physical device_map), which overflows
    // the default stack on Windows (1MB). We spawn a thread with a larger stack to create it.
    let mut machine = std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(move || Box::new(Machine::new(cfg)))
        .unwrap()
        .join()
        .unwrap();
    machine.register_system_controller();

    // CI control socket: started after Machine::new so it can hand out the
    // machine pointer + CiSerialBackend to command handlers.
    #[cfg(unix)]
    let _ci_server = if ci_enabled {
        let mptr: *mut iris::machine::Machine = &mut *machine;
        match iris::ci::start_server(mptr, &ci_socket_path) {
            Ok(s) => Some(s),
            Err(e) => {
                eprintln!("iris: failed to start CI server: {}", e);
                std::process::exit(1);
            }
        }
    } else {
        None
    };

    // DIAG: optionally enable verbose logging from startup via IRIS_DEBUG_LOG.
    // IRIS_DEBUG_LOG="mc,mips" enables those modules. "all" enables everything.
    // Output is broadcast to a stderr sink so jit-diag.sh's tee captures it inline.
    if let Ok(spec) = std::env::var("IRIS_DEBUG_LOG") {
        if let Some(dl) = iris::devlog::DEVLOG.get() {
            // Register stderr as a sink so dlog output reaches our captured log.
            let stderr_sink: iris::devlog::DevLogWriter = std::sync::Arc::new(
                parking_lot::Mutex::new(std::io::stderr()),
            );
            dl.add_sink(stderr_sink);

            for name in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                if name == "all" {
                    for m in iris::devlog::LogModule::all() { dl.enable(*m); }
                    eprintln!("DIAG: enabled all log modules -> stderr");
                } else if let Some(m) = iris::devlog::LogModule::from_str(name) {
                    dl.enable(m);
                    eprintln!("DIAG: enabled log module {} -> stderr", m.name());
                } else {
                    eprintln!("DIAG: unknown log module '{}'", name);
                }
            }
        }
    }

    // Start GDB stub before starting the CPU so that in developer mode (CPU not
    // auto-started), GDB can connect and set breakpoints before running.
    if let Some(port) = gdb_port {
        let cpu_debug = machine.get_cpu_debug();
        iris::gdb_stub::start_gdb_server(port, cpu_debug);
    }

    machine.start();
    if !ci_enabled {
        std::thread::spawn(|| {
            Machine::run_console_client();
        });
    }

    let show_window = !headless && !(ci_enabled && !ci_display);
    if !show_window {
        if headless {
            eprintln!("iris: running headless (no REX3, no window)");
        } else if ci_enabled {
            eprintln!("iris: --ci mode (REX3 rendering to offscreen buffer, no window)");
        }
        // Park the main thread so background threads (CPU, REX3 refresh,
        // CI socket) keep running. `quit` via the CI socket calls
        // std::process::exit.
        std::thread::park();
    } else {
        use iris::ui::Ui;
        use winit::event_loop::EventLoop;
        let event_loop = EventLoop::new().unwrap();
        let rex3 = machine.get_rex3().expect("rex3 must be present in non-headless mode");
        let ui = Ui::new(machine.get_ps2(), rex3, machine.get_timer_manager(), &event_loop, scale, scroll_pixels_per_line, lock_aspect_ratio);
        ui.run(event_loop);
    }

    machine.stop();
}

/// Print which compile-time feature flags this binary was built with. Handy
/// when diagnosing behaviour that depends on the build (e.g. MIPS `jit` bypasses
/// the interpreter's idle-park path, so an idle guest spins the host CPU).
fn print_build_features() {
    const FEATURES: &[(&str, bool)] = &[
        ("jit", cfg!(feature = "jit")),
        ("rex-jit", cfg!(feature = "rex-jit")),
        ("lightning", cfg!(feature = "lightning")),
        ("tlbvmap", cfg!(feature = "tlbvmap")),
        ("tlbstats", cfg!(feature = "tlbstats")),
        ("chd", cfg!(feature = "chd")),
        ("camera", cfg!(feature = "camera")),
        ("ci_clock", cfg!(feature = "ci_clock")),
        ("developer", cfg!(feature = "developer")),
        ("developer_ip7", cfg!(feature = "developer_ip7")),
        ("debug_cache", cfg!(feature = "debug_cache")),
    ];
    let on: Vec<&str> = FEATURES.iter().filter(|(_, e)| *e).map(|(n, _)| *n).collect();
    eprintln!(
        "iris: build features: {}",
        if on.is_empty() { "(none)".to_string() } else { on.join(" ") }
    );
}

