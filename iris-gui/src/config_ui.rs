use egui::{Color32, ComboBox, DragValue, Grid, RichText, ScrollArea, TextEdit, Ui};
use iris::build_features;
use std::path::Path;
use iris::config::{
    ForwardBind, ForwardProto, MachineConfig, NetMode, NfsConfig, PortForwardConfig,
    ScsiDeviceConfig, VinoSource, VinoStandard, VALID_BANK_SIZES,
};

/// A host network interface candidate for the PCAP backend selector. This is a
/// GUI-local, feature-independent copy of `iris::net_pcap::NetInterface` so the
/// App state and `show_network` signature don't need `#[cfg(feature = "pcap")]`.
#[derive(Debug, Clone)]
pub struct PcapIface {
    pub name: String,
    pub description: Option<String>,
    pub addrs: Vec<String>,
    pub up: bool,
    pub running: bool,
    pub loopback: bool,
}

impl PcapIface {
    /// One-line summary for the dropdown row.
    fn summary(&self) -> String {
        let mut s = self.name.clone();
        let mut tags = Vec::new();
        if self.up { tags.push("up"); }
        if self.running { tags.push("running"); }
        if self.loopback { tags.push("loopback"); }
        if !tags.is_empty() {
            s.push_str(&format!("  [{}]", tags.join(",")));
        }
        if let Some(ip) = self.addrs.first() {
            s.push_str(&format!("  {ip}"));
        }
        // Windows device names are opaque GUIDs; the description (NIC model) is
        // far more useful, so append it when present.
        if let Some(desc) = self.description.as_deref().filter(|d| !d.is_empty()) {
            s.push_str(&format!("  — {desc}"));
        }
        s
    }
}

/// Enumerate host interfaces for the PCAP selector. Returns the candidate list,
/// or an error string (insufficient privileges / no driver / feature missing).
/// Only does real work when built with `--features pcap`; otherwise returns a
/// hint so the UI can explain why the dropdown is unavailable.
pub fn enumerate_pcap_ifaces() -> Result<Vec<PcapIface>, String> {
    #[cfg(feature = "pcap")]
    {
        iris::net_pcap::list_interfaces().map(|list| {
            list.into_iter()
                .map(|i| PcapIface {
                    name: i.name,
                    description: i.description,
                    addrs: i.addresses.iter().map(|a| a.to_string()).collect(),
                    up: i.up,
                    running: i.running,
                    loopback: i.loopback,
                })
                .collect()
        })
    }
    #[cfg(not(feature = "pcap"))]
    {
        Err("this build lacks --features pcap; rebuild iris-gui with `--features pcap` \
             to enumerate and bridge onto host interfaces"
            .to_string())
    }
}

/// Which config tab is focused. Toolbar quick-buttons set this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    General,
    Disks,
    Network,
    Memory,
    Display,
    VideoIn,
    Debug,
    Ci,
}

impl Tab {
    /// Tabs to show for the active build. The Debug/JIT tab is hidden in
    /// lightning builds (the JIT debug paths it drives are compiled out), and
    /// the CI/Automation tab is hidden in App Store builds (the iris-ci socket
    /// is a developer automation feature, not something a sandboxed end user
    /// can use). Both fall back to the full set for ordinary builds.
    pub fn visible() -> Vec<Tab> {
        let mut tabs = vec![
            Tab::General, Tab::Disks, Tab::Network, Tab::Memory,
            Tab::Display, Tab::VideoIn,
        ];
        if !build_features::LIGHTNING {
            tabs.push(Tab::Debug);
        }
        if !cfg!(feature = "appstore") {
            tabs.push(Tab::Ci);
        }
        tabs
    }
    pub fn label(self) -> &'static str {
        match self {
            Tab::General => "General",
            Tab::Disks   => "Disks",
            Tab::Network => "Networking",
            Tab::Memory  => "Memory",
            Tab::Display => "Display",
            Tab::VideoIn => "Video-In",
            Tab::Debug   => "Debug / JIT",
            Tab::Ci      => "CI / Automation",
        }
    }
}

/// IRIS_JIT* environment variables exposed as GUI fields. These get exported
/// into the process env before `Machine::new` is called (whether iris is
/// hosted in-process or spawned). All optional; empty means "leave default".
#[derive(Debug, Clone, Default)]
pub struct JitEnv {
    pub iris_jit: bool,
    pub max_tier: Option<u8>,
    pub verify: bool,
    pub no_stores: bool,
    pub probe: String,
    pub trace_file: String,
    pub profile_file: String,
    pub no_idle: bool,
    pub debug_log: String,
}

impl JitEnv {
    /// Apply to current process env. Called by iris-gui before Machine::new.
    pub fn export(&self) {
        if self.iris_jit { std::env::set_var("IRIS_JIT", "1"); }
        if let Some(t) = self.max_tier { std::env::set_var("IRIS_JIT_MAX_TIER", t.to_string()); }
        if self.verify    { std::env::set_var("IRIS_JIT_VERIFY", "1"); }
        if self.no_stores { std::env::set_var("IRIS_JIT_NO_STORES", "1"); }
        if !self.probe.is_empty()         { std::env::set_var("IRIS_JIT_PROBE", &self.probe); }
        if !self.trace_file.is_empty()    { std::env::set_var("IRIS_JIT_TRACE", &self.trace_file); }
        if !self.profile_file.is_empty()  { std::env::set_var("IRIS_JIT_PROFILE", &self.profile_file); }
        if self.no_idle { std::env::set_var("IRIS_NO_IDLE", "1"); }
        if !self.debug_log.is_empty() { std::env::set_var("IRIS_DEBUG_LOG", &self.debug_log); }
    }
}

/// Action a config tab asks the app to perform that needs app-level state
/// (e.g. a confirmation modal) the immediate-mode tab UI doesn't own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConfigAction {
    #[default]
    None,
    /// User clicked "Use embedded PROM"; the app should confirm with the user
    /// and, if accepted, clear `cfg.prom` (an empty path falls back to the
    /// built-in PROM in `iris::prom::Prom::from_file_or_embedded`).
    RequestEmbeddedProm,
    /// User clicked "Test Camera" on the Video-In tab; the app should open the
    /// host camera and show a live preview (using the current `[vino]` standard
    /// and camera index).
    TestCamera,
    /// User clicked "Refresh" on the Network tab's PCAP selector; the app should
    /// re-enumerate host interfaces and update its cache.
    RefreshPcapIfaces,
}

pub fn show_tab(
    ui: &mut Ui,
    tab: Tab,
    cfg: &mut MachineConfig,
    jit: &mut JitEnv,
    pcap_ifaces: &Option<Result<Vec<PcapIface>, String>>,
) -> ConfigAction {
    ScrollArea::vertical().show(ui, |ui| match tab {
        Tab::General => show_general(ui, cfg),
        Tab::Disks   => { show_disks(ui, cfg); ConfigAction::None }
        Tab::Network => show_network(ui, cfg, pcap_ifaces),
        Tab::Memory  => { show_memory(ui, cfg); ConfigAction::None }
        Tab::Display => { show_display(ui, cfg); ConfigAction::None }
        Tab::VideoIn => show_vino(ui, cfg),
        Tab::Debug   => { show_debug(ui, cfg, jit); ConfigAction::None }
        Tab::Ci      => { show_ci(ui, cfg); ConfigAction::None }
    }).inner
}

fn show_general(ui: &mut Ui, cfg: &mut MachineConfig) -> ConfigAction {
    let mut action = ConfigAction::None;
    ui.heading("General");
    Grid::new("general_grid").num_columns(2).striped(true).show(ui, |ui| {
        ui.label("PROM image");
        path_row(ui, "prom", &mut cfg.prom, Pick::OpenFile, PROM_FILTERS);
        ui.end_row();

        // Leaving the PROM path empty boots the built-in PROM. Expose that as
        // an explicit button so reverting from a (possibly missing) custom PROM
        // is discoverable instead of "delete the text by hand". Disabled when
        // already empty, so the confirm prompt only ever appears when a custom
        // PROM is selected.
        ui.label("");
        ui.horizontal(|ui| {
            let custom = !cfg.prom.is_empty();
            if ui.add_enabled(custom, egui::Button::new("Use embedded PROM"))
                .on_hover_text("Boot IRIS's built-in PROM instead of a file")
                .clicked()
            {
                action = ConfigAction::RequestEmbeddedProm;
            }
            if !custom {
                ui.label(RichText::new("(using built-in PROM)").weak());
            }
        });
        ui.end_row();

        ui.label("NVRAM file");
        path_row(ui, "nvram", &mut cfg.nvram, Pick::SaveFile, NVRAM_FILTERS);
        ui.end_row();

        ui.label("Serial log (ttyd1 -> file)");
        path_row_opt(ui, "serial_log", &mut cfg.serial_log, Pick::SaveFile, ANY_FILTERS);
        ui.end_row();
    });
    action
}

fn show_memory(ui: &mut Ui, cfg: &mut MachineConfig) {
    ui.heading("Memory");
    ui.label("RAM bank sizes in MB (valid: 0, 8, 16, 32, 64, 128)");
    Grid::new("mem_grid").num_columns(2).striped(true).show(ui, |ui| {
        for i in 0..4 {
            ui.label(format!("Bank {i}"));
            let cur = cfg.banks[i];
            ComboBox::from_id_salt(("bank", i)).selected_text(format!("{cur} MB"))
                .show_ui(ui, |ui| {
                    for &sz in VALID_BANK_SIZES {
                        ui.selectable_value(&mut cfg.banks[i], sz, format!("{sz} MB"));
                    }
                });
            ui.end_row();
        }
    });
    let total: u32 = cfg.banks.iter().sum();
    ui.label(format!("Total: {total} MB"));
}

fn show_display(ui: &mut Ui, cfg: &mut MachineConfig) {
    ui.heading("Display");
    Grid::new("disp_grid").num_columns(2).striped(true).show(ui, |ui| {
        ui.label("Window scale");
        ComboBox::from_id_salt("scale").selected_text(format!("{}×", cfg.scale))
            .show_ui(ui, |ui| {
                for s in 1u32..=4 {
                    ui.selectable_value(&mut cfg.scale, s, format!("{s}×"));
                }
            });
        ui.end_row();

        ui.label("Headless (no REX3 graphics)");
        ui.checkbox(&mut cfg.headless, "");
        ui.end_row();

        ui.label("No audio (disable HAL2)");
        ui.checkbox(&mut cfg.no_audio, "");
        ui.end_row();
    });
}

fn show_disks(ui: &mut Ui, cfg: &mut MachineConfig) {
    ui.heading("SCSI devices");
    ui.horizontal(|ui| {
        ui.label("IDs 1–7. CD-ROMs typically use 4–6.");
        if build_features::CHD {
            ui.label(RichText::new("[CHD support: ON]").color(Color32::LIGHT_GREEN).small());
        } else {
            ui.label(RichText::new("[CHD support: OFF — rebuild with --features chd]")
                .color(Color32::from_rgb(220, 170, 90)).small());
        }
    });
    let mut to_delete: Option<u8> = None;
    for id in 1u8..=7 {
        ui.separator();
        let exists = cfg.scsi.contains_key(&id);
        ui.horizontal(|ui| {
            ui.strong(format!("scsi{id}"));
            if exists {
                if ui.button("Remove").clicked() {
                    to_delete = Some(id);
                }
            } else if ui.button("Attach…").clicked() {
                cfg.scsi.insert(id, ScsiDeviceConfig {
                    path: format!("scsi{id}.raw"),
                    discs: vec![],
                    cdrom: false,
                    overlay: false,
                    scratch: false,
                    size_mb: None,
                });
            }
        });
        if let Some(dev) = cfg.scsi.get_mut(&id) {
            Grid::new(("scsi_grid", id)).num_columns(2).striped(true).show(ui, |ui| {
                ui.label("Image path");
                path_row(ui, ("scsi_path", id), &mut dev.path,
                    if dev.scratch { Pick::SaveFile } else { Pick::OpenFile },
                    DISK_FILTERS);
                ui.end_row();
                if dev.path.ends_with(".chd") && !build_features::CHD {
                    ui.label("");
                    ui.label(RichText::new("⚠ .chd path but this build lacks CHD support — rebuild with --features chd")
                        .color(Color32::from_rgb(230, 140, 70)));
                    ui.end_row();
                }

                ui.label("Type");
                let was_cd = dev.cdrom;
                let mut is_cd = dev.cdrom;
                ComboBox::from_id_salt(("type", id))
                    .selected_text(if is_cd { "CD-ROM" } else { "HDD" })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut is_cd, false, "HDD");
                        ui.selectable_value(&mut is_cd, true, "CD-ROM");
                    });
                // Switching to CD-ROM defaults to an empty drive (no media):
                // clear the auto-generated HDD placeholder path so it doesn't
                // look like a (missing) disc. Load media via "Insert disc…" in
                // the SCSI menu, or just type a path here.
                if is_cd && !was_cd && dev.path == format!("scsi{id}.raw") {
                    dev.path.clear();
                }
                dev.cdrom = is_cd;
                ui.end_row();
                if dev.cdrom && dev.path.is_empty() {
                    ui.label("");
                    ui.label(RichText::new("empty drive (no media) — insert a disc via the SCSI menu")
                        .weak().small());
                    ui.end_row();
                }

                ui.label("Overlay (COW writes -> .overlay)");
                ui.checkbox(&mut dev.overlay, "");
                ui.end_row();

                ui.label("Scratch volume");
                ui.checkbox(&mut dev.scratch, "");
                ui.end_row();

                if dev.scratch {
                    ui.label("Scratch size (MB)");
                    let mut sz = dev.size_mb.unwrap_or(64);
                    if ui.add(DragValue::new(&mut sz).range(1..=8192)).changed() {
                        dev.size_mb = Some(sz);
                    }
                    ui.end_row();
                }
            });

            if dev.cdrom {
                ui.label("Extra changer discs:");
                let mut drop_idx: Option<usize> = None;
                for (i, disc) in dev.discs.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        path_row(ui, ("disc", id, i), disc, Pick::OpenFile, DISK_FILTERS);
                        if ui.button("×").clicked() { drop_idx = Some(i); }
                    });
                }
                if let Some(i) = drop_idx { dev.discs.remove(i); }
                if ui.button("+ Add disc").clicked() {
                    dev.discs.push(String::new());
                }
            }
        }
    }
    if let Some(id) = to_delete { cfg.scsi.remove(&id); }
}

fn show_network(
    ui: &mut Ui,
    cfg: &mut MachineConfig,
    pcap_ifaces: &Option<Result<Vec<PcapIface>, String>>,
) -> ConfigAction {
    let mut action = ConfigAction::None;
    ui.heading("Networking");

    // The backend selector (and the entire PCAP UI) is only shown when this
    // build actually has PCAP support. App Store / bundled builds compile
    // without `--features pcap`, where NAT is the only backend — so PCAP must
    // not appear anywhere in the UI (a dangling, non-functional option risks an
    // App Store rejection). Such builds also force NAT at runtime regardless of
    // a stale `mode = "pcap"` carried in from an imported config.
    if build_features::PCAP {
        Grid::new("net_mode_grid").num_columns(2).striped(true).show(ui, |ui| {
            ui.label("Backend");
            ComboBox::from_id_salt("net_mode")
                .selected_text(match cfg.network.mode {
                    NetMode::Nat  => "NAT gateway",
                    NetMode::Pcap => "PCAP (bridged)",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut cfg.network.mode, NetMode::Nat, "NAT gateway");
                    ui.selectable_value(&mut cfg.network.mode, NetMode::Pcap, "PCAP (bridged)");
                });
            ui.end_row();
        });

        if cfg.network.mode == NetMode::Pcap {
            if let a @ ConfigAction::RefreshPcapIfaces = pcap_interface_picker(ui, cfg, pcap_ifaces) {
                action = a;
            }

            ui.colored_label(Color32::from_rgb(0xd0, 0xa0, 0x40),
                "PCAP mode bridges onto a real interface. NAT, port forwards, and NFS \
                 below are ignored; the guest uses your real LAN. Requires elevated \
                 privileges (root/CAP_NET_RAW on Unix, or a WinPcap-compatible driver \
                 + Administrator on Windows).");
            ui.separator();
        }
    }

    Grid::new("nat_grid").num_columns(2).striped(true).show(ui, |ui| {
        ui.label("NAT subnet (CIDR)");
        let mut s = cfg.nat_subnet.clone().unwrap_or_default();
        if ui.add(TextEdit::singleline(&mut s).hint_text("192.168.0.0/24").desired_width(220.0)).changed() {
            cfg.nat_subnet = if s.is_empty() { None } else { Some(s) };
        }
        ui.end_row();
    });

    ui.separator();
    ui.strong("Port forwards");
    let mut drop: Option<usize> = None;
    for (i, pf) in cfg.port_forward.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            ComboBox::from_id_salt(("proto", i))
                .selected_text(match pf.proto { ForwardProto::Tcp => "tcp", ForwardProto::Udp => "udp" })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut pf.proto, ForwardProto::Tcp, "tcp");
                    ui.selectable_value(&mut pf.proto, ForwardProto::Udp, "udp");
                });
            ui.label("host");
            ui.add(DragValue::new(&mut pf.host_port).range(1..=65535));
            ui.label("-> guest");
            ui.add(DragValue::new(&mut pf.guest_port).range(1..=65535));
            ComboBox::from_id_salt(("bind", i))
                .selected_text(match pf.bind { ForwardBind::Localhost => "localhost", ForwardBind::Any => "any" })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut pf.bind, ForwardBind::Localhost, "localhost");
                    ui.selectable_value(&mut pf.bind, ForwardBind::Any, "any");
                });
            if ui.button("×").clicked() { drop = Some(i); }
        });
    }
    if let Some(i) = drop { cfg.port_forward.remove(i); }
    if ui.button("+ Add forward").clicked() {
        cfg.port_forward.push(PortForwardConfig {
            proto: ForwardProto::Tcp, host_port: 0, guest_port: 0, bind: ForwardBind::Localhost,
        });
    }

    ui.separator();
    ui.strong("NFS share");
    let mut has_nfs = cfg.nfs.is_some();
    if ui.checkbox(&mut has_nfs, "Enable NFS").changed() {
        cfg.nfs = if has_nfs {
            Some(NfsConfig {
                shared_dir: String::new(),
                unfsd: "unfsd".into(),
                nfs_host_port: 12049,
                mountd_host_port: 11234,
            })
        } else { None };
    }
    if let Some(nfs) = cfg.nfs.as_mut() {
        Grid::new("nfs_grid").num_columns(2).striped(true).show(ui, |ui| {
            ui.label("Shared dir");
            path_row(ui, "nfs_shared", &mut nfs.shared_dir, Pick::Dir, ANY_FILTERS);
            ui.end_row();
            ui.label("unfsd binary");
            path_row(ui, "nfs_unfsd", &mut nfs.unfsd, Pick::OpenFile, ANY_FILTERS);
            ui.end_row();
            ui.label("NFS host port");
            ui.add(DragValue::new(&mut nfs.nfs_host_port).range(1..=65535));
            ui.end_row();
            ui.label("mountd host port");
            ui.add(DragValue::new(&mut nfs.mountd_host_port).range(1..=65535));
            ui.end_row();
        });
    }

    action
}

/// PCAP interface picker: a dropdown of enumerated host interfaces (with an
/// "Auto-pick" entry and a "Manual…" escape hatch), plus a Refresh button.
/// Stores the choice by interface *name* in `cfg.network.pcap_interface`
/// (`None` = auto-pick). Returns `RefreshPcapIfaces` when the user asks to
/// re-enumerate.
fn pcap_interface_picker(
    ui: &mut Ui,
    cfg: &mut MachineConfig,
    pcap_ifaces: &Option<Result<Vec<PcapIface>, String>>,
) -> ConfigAction {
    let mut action = ConfigAction::None;

    // Selected text for the combo: the current name, "Auto-pick", or the raw
    // value if it's something not in the list (e.g. an index or manual name).
    let current = cfg.network.pcap_interface.clone();
    let selected_text = match &current {
        None => "Auto-pick (first up, non-loopback)".to_string(),
        Some(v) if v.is_empty() => "Auto-pick (first up, non-loopback)".to_string(),
        Some(v) => v.clone(),
    };

    ui.horizontal(|ui| {
        ui.label("PCAP interface");

        ComboBox::from_id_salt("pcap_iface")
            .selected_text(selected_text)
            .width(320.0)
            .show_ui(ui, |ui| {
                // Auto-pick entry.
                let mut is_auto = current.as_deref().unwrap_or("").is_empty();
                if ui.selectable_label(is_auto, "Auto-pick (first up, non-loopback)").clicked() {
                    cfg.network.pcap_interface = None;
                    is_auto = true;
                }
                let _ = is_auto;

                match pcap_ifaces {
                    Some(Ok(list)) if !list.is_empty() => {
                        ui.separator();
                        for iface in list {
                            let selected = current.as_deref() == Some(iface.name.as_str());
                            if ui.selectable_label(selected, iface.summary()).clicked() {
                                cfg.network.pcap_interface = Some(iface.name.clone());
                            }
                        }
                    }
                    Some(Ok(_)) => {
                        ui.separator();
                        ui.label(RichText::new("(no interfaces enumerated)").weak());
                    }
                    Some(Err(e)) => {
                        ui.separator();
                        ui.label(RichText::new(format!("(cannot list: {e})")).weak());
                    }
                    None => {
                        ui.separator();
                        ui.label(RichText::new("(click Refresh to enumerate)").weak());
                    }
                }
            });

        if ui.button("⟳ Refresh").on_hover_text("Re-enumerate host interfaces").clicked() {
            action = ConfigAction::RefreshPcapIfaces;
        }
    });

    // Manual entry escape hatch: lets the user type an index ("1"), an exact
    // name, or a Windows \Device\NPF_{...} string the dropdown can't show well.
    ui.horizontal(|ui| {
        ui.label("   or type index/name");
        let mut manual = current.clone().unwrap_or_default();
        if ui.add(TextEdit::singleline(&mut manual)
            .hint_text("e.g. 1, eth0, or blank = auto")
            .desired_width(260.0)).changed()
        {
            cfg.network.pcap_interface = if manual.trim().is_empty() { None } else { Some(manual) };
        }
    });

    // Show an inline error if enumeration failed.
    if let Some(Err(e)) = pcap_ifaces {
        ui.colored_label(Color32::from_rgb(0xe0, 0x60, 0x60), format!("Interface list unavailable: {e}"));
    }

    action
}

fn show_vino(ui: &mut Ui, cfg: &mut MachineConfig) -> ConfigAction {
    let mut action = ConfigAction::None;
    ui.heading("Video-In (IndyCam)");
    Grid::new("vino_grid").num_columns(2).striped(true).show(ui, |ui| {
        ui.label("Source");
        ComboBox::from_id_salt("vino_src")
            .selected_text(match cfg.vino.source {
                VinoSource::Camera      => "camera",
                VinoSource::TestPattern => "test_pattern",
                VinoSource::Black       => "black",
                VinoSource::Off         => "off (disabled)",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut cfg.vino.source, VinoSource::Off, "off (disabled)");
                ui.selectable_value(&mut cfg.vino.source, VinoSource::TestPattern, "test_pattern");
                let camera_label = if build_features::CAMERA {
                    "camera"
                } else {
                    "camera (needs --features camera)"
                };
                ui.selectable_value(&mut cfg.vino.source, VinoSource::Camera, camera_label);
                ui.selectable_value(&mut cfg.vino.source, VinoSource::Black, "black");
            });
        ui.end_row();

        ui.label("Standard");
        ComboBox::from_id_salt("vino_std")
            .selected_text(match cfg.vino.standard { VinoStandard::Ntsc => "ntsc", VinoStandard::Pal => "pal" })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut cfg.vino.standard, VinoStandard::Ntsc, "ntsc");
                ui.selectable_value(&mut cfg.vino.standard, VinoStandard::Pal, "pal");
            });
        ui.end_row();

        ui.label("Camera index");
        ui.add(DragValue::new(&mut cfg.vino.camera_index).range(0..=15));
        ui.end_row();
    });

    // Live host-camera test: opens the selected camera directly (no IRIX boot
    // needed) so the user can confirm the capture path works — and, on macOS,
    // grant the camera permission. This exercises the same host-capture code
    // the VINO/IndyCam source uses.
    ui.add_space(8.0);
    if build_features::CAMERA {
        ui.horizontal(|ui| {
            if ui.button("📷 Test Camera").clicked() {
                action = ConfigAction::TestCamera;
            }
            ui.label(
                RichText::new(format!(
                    "Preview host camera #{} live ({}).",
                    cfg.vino.camera_index,
                    match cfg.vino.standard { VinoStandard::Ntsc => "NTSC", VinoStandard::Pal => "PAL" },
                ))
                .weak(),
            );
        });
        ui.label(
            RichText::new(
                "On first use macOS will ask for camera permission. The camera \
                 is released when you close the preview.",
            )
            .weak()
            .small(),
        );
    } else {
        ui.label(
            RichText::new("Camera test unavailable — this build was compiled without --features camera.")
                .weak(),
        );
    }

    action
}

fn show_debug(ui: &mut Ui, cfg: &mut MachineConfig, jit: &mut JitEnv) {
    ui.heading("Debug / JIT");
    if build_features::LIGHTNING {
        ui.label(RichText::new(
            "⚡ Lightning build — interactive debugging is disabled \
             (no breakpoints, no GDB stub, no traceback). Rebuild without \
             --features lightning to re-enable.").color(Color32::from_rgb(220, 170, 90)));
        ui.separator();
    } else {
        Grid::new("dbg_grid").num_columns(2).striped(true).show(ui, |ui| {
            ui.label("GDB stub port");
            let mut port = cfg.gdb_port.unwrap_or(0);
            if ui.add(DragValue::new(&mut port).range(0..=65535)).changed() {
                cfg.gdb_port = if port == 0 { None } else { Some(port) };
            }
            ui.end_row();
        });
    }
    ui.separator();
    ui.label("JIT (requires `cargo build --features jit`)");
    Grid::new("jit_grid").num_columns(2).striped(true).show(ui, |ui| {
        ui.label("Enable JIT (IRIS_JIT=1)");
        ui.checkbox(&mut jit.iris_jit, "");
        ui.end_row();

        ui.label("Max tier (0=ALU, 1=Loads, 2=Full)");
        let mut t = jit.max_tier.unwrap_or(2);
        if ui.add(DragValue::new(&mut t).range(0..=2)).changed() {
            jit.max_tier = Some(t);
        }
        ui.end_row();

        ui.label("Verify against interpreter");
        ui.checkbox(&mut jit.verify, "");
        ui.end_row();

        ui.label("Disable JIT stores (diagnostic)");
        ui.checkbox(&mut jit.no_stores, "");
        ui.end_row();

        ui.label("Probe interval");
        ui.add(TextEdit::singleline(&mut jit.probe).hint_text("default 200").desired_width(120.0));
        ui.end_row();

        ui.label("Trace file");
        path_row(ui, "jit_trace", &mut jit.trace_file, Pick::SaveFile, ANY_FILTERS);
        ui.end_row();

        ui.label("Profile file");
        path_row(ui, "jit_profile", &mut jit.profile_file, Pick::SaveFile, ANY_FILTERS);
        ui.end_row();
    });
    ui.separator();
    Grid::new("misc_grid").num_columns(2).striped(true).show(ui, |ui| {
        ui.label("Disable idle park (IRIS_NO_IDLE)");
        ui.checkbox(&mut jit.no_idle, "");
        ui.end_row();
        ui.label("Devlog spec (IRIS_DEBUG_LOG)");
        ui.add(TextEdit::singleline(&mut jit.debug_log).hint_text("all, or e.g. mc,mips").desired_width(280.0));
        ui.end_row();
    });
}

fn show_ci(ui: &mut Ui, cfg: &mut MachineConfig) {
    ui.heading("CI / Automation");
    Grid::new("ci_grid").num_columns(2).striped(true).show(ui, |ui| {
        ui.label("Enable CI mode");
        ui.checkbox(&mut cfg.ci, "");
        ui.end_row();
        ui.label("CI socket path");
        path_row(ui, "ci_socket", &mut cfg.ci_socket, Pick::SaveFile, SOCKET_FILTERS);
        ui.end_row();
        ui.label("Keep window visible (--ci-display)");
        ui.checkbox(&mut cfg.ci_display, "");
        ui.end_row();
    });
}

/// Serialize `cfg` back to TOML string in the same style as iris.toml.
pub fn cfg_to_toml(cfg: &MachineConfig) -> Result<String, String> {
    toml::to_string_pretty(cfg).map_err(|e| e.to_string())
}

/// How a Browse button should pick a path.
#[derive(Clone, Copy)]
enum Pick {
    OpenFile,
    SaveFile,
    Dir,
}

/// A TextEdit + 📁 Browse button that updates `value` in place.
/// `filters` is a list of (label, &[extensions]); ignored for `Pick::Dir`.
fn path_row(
    ui: &mut Ui,
    id: impl std::hash::Hash,
    value: &mut String,
    mode: Pick,
    filters: &[(&str, &[&str])],
) {
    ui.push_id(id, |ui| {
        ui.horizontal(|ui| {
            ui.add(TextEdit::singleline(value).desired_width(320.0));
            if ui.button("📁").on_hover_text("Browse…").clicked() {
                let mut d = rfd::FileDialog::new();
                // Start the dialog in the existing path's directory if any.
                if !value.is_empty() {
                    let p = Path::new(value);
                    if let Some(parent) = p.parent() {
                        if parent.as_os_str().len() > 0 && parent.exists() {
                            d = d.set_directory(parent);
                        }
                    }
                    if let Some(name) = p.file_name() {
                        d = d.set_file_name(name.to_string_lossy());
                    }
                }
                if matches!(mode, Pick::OpenFile | Pick::SaveFile) {
                    for (label, exts) in filters {
                        d = d.add_filter(*label, exts);
                    }
                }
                let picked = match mode {
                    Pick::OpenFile => d.pick_file(),
                    Pick::SaveFile => d.save_file(),
                    Pick::Dir      => d.pick_folder(),
                };
                if let Some(p) = picked {
                    *value = p.to_string_lossy().into_owned();
                }
            }
        });
    });
}

/// Same as `path_row` but for `Option<String>` — Browse populates Some,
/// the user can clear by emptying the text.
fn path_row_opt(
    ui: &mut Ui,
    id: impl std::hash::Hash,
    value: &mut Option<String>,
    mode: Pick,
    filters: &[(&str, &[&str])],
) {
    let mut s = value.clone().unwrap_or_default();
    path_row(ui, id, &mut s, mode, filters);
    *value = if s.is_empty() { None } else { Some(s) };
}

/// Common file filters.
const PROM_FILTERS:   &[(&str, &[&str])] = &[("PROM image", &["bin"]), ("All", &["*"])];
const NVRAM_FILTERS:  &[(&str, &[&str])] = &[("NVRAM",      &["bin"]), ("All", &["*"])];
const DISK_FILTERS:   &[(&str, &[&str])] = &[
    ("Disk images", &["raw", "img", "chd"]),
    ("ISO images",  &["iso"]),
    ("All",         &["*"]),
];
const ANY_FILTERS:    &[(&str, &[&str])] = &[("All", &["*"])];
const SOCKET_FILTERS: &[(&str, &[&str])] = &[("Unix socket", &["sock"]), ("All", &["*"])];

