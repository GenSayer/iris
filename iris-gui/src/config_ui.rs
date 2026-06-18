use egui::{Color32, ComboBox, DragValue, Grid, RichText, ScrollArea, TextEdit, Ui};
use iris::build_features;
use std::path::Path;
use iris::config::{
    ForwardBind, ForwardProto, MachineConfig, NfsConfig, PortForwardConfig,
    ScsiDeviceConfig, VinoSource, VinoStandard, VALID_BANK_SIZES,
};

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
}

/// Everything a config tab hands back to the app for one frame.
#[derive(Default)]
pub struct TabOutcome {
    pub action: ConfigAction,
    pub net: NetworkOutcome,
}

pub fn show_tab(
    ui: &mut Ui,
    tab: Tab,
    cfg: &mut MachineConfig,
    jit: &mut JitEnv,
    host: &[crate::netplan::HostIface],
) -> TabOutcome {
    ScrollArea::vertical().show(ui, |ui| match tab {
        Tab::General => TabOutcome { action: show_general(ui, cfg), ..Default::default() },
        Tab::Disks   => { show_disks(ui, cfg); TabOutcome::default() }
        Tab::Network => TabOutcome { net: show_network(ui, cfg, host), ..Default::default() },
        Tab::Memory  => { show_memory(ui, cfg); TabOutcome::default() }
        Tab::Display => { show_display(ui, cfg); TabOutcome::default() }
        Tab::VideoIn => TabOutcome { action: show_vino(ui, cfg), ..Default::default() },
        Tab::Debug   => { show_debug(ui, cfg, jit); TabOutcome::default() }
        Tab::Ci      => { show_ci(ui, cfg); TabOutcome::default() }
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

/// A soft-invalid subnet the user just entered, surfaced to the app so it can
/// pop the "Override Sanity Checks / Cancel" confirmation modal.
pub struct NetSanityPrompt {
    /// Why it's questionable (non-RFC1918 or a host-network conflict).
    pub reason: String,
    /// A known-good CIDR offered as the safe alternative.
    pub suggestion: String,
    /// The `nat_subnet` value before this edit, restored if the user cancels.
    pub revert_to: Option<String>,
}

/// What the Networking tab asks the app to do beyond mutating `cfg`. Routed up
/// through [`show_tab`] because the immediate-mode tab can't own app-level state
/// (the dirty flag, the confirmation modal).
#[derive(Default)]
pub struct NetworkOutcome {
    /// A networking field changed this frame → the app should mark cfg dirty.
    pub changed: bool,
    /// A soft-invalid subnet was just committed → pop the override modal.
    pub prompt: Option<NetSanityPrompt>,
}

fn show_network(ui: &mut Ui, cfg: &mut MachineConfig, host: &[crate::netplan::HostIface]) -> NetworkOutcome {
    use crate::netplan;
    let mut out = NetworkOutcome::default();
    ui.heading("Networking");

    ui.label(RichText::new(
        "IRIS gives the Indy its own private NAT network, the same trick your home router uses. \
         The Indy reaches the internet through IRIS, but nothing on your real network can see it. \
         Pick a subnet that does not overlap a network your computer already uses (Wi-Fi, Ethernet, \
         VPN, Docker, etc.). If it does, IRIS flags it below.")
        .weak());
    ui.add_space(6.0);

    // The UI exposes the network base address (a plain IPv4, not CIDR) and the
    // mask separately; they compose into `cfg.nat_subnet` (a CIDR string the
    // backend wants, always snapped to a clean network address). The custom-mode
    // flags and the base text buffer persist in egui memory so partial typing
    // and the "Custom" reveal survive across frames; `last_id` tracks the value
    // we last stored so an external change (Cancel revert, machine switch)
    // re-syncs the controls.
    let before = cfg.nat_subnet.clone();
    let (base0, prefix0) = netplan::parse_cidr(cfg.nat_subnet.as_deref());

    let net_custom_id  = ui.make_persistent_id("net_net_custom");
    let mask_custom_id = ui.make_persistent_id("net_mask_custom");
    let base_buf_id    = ui.make_persistent_id("net_base_buf");
    let last_id        = ui.make_persistent_id("net_last_composed");

    let preset_mask = |p: u8| netplan::MASK_PRESETS.contains(&p);
    let mut net_custom:  bool = ui.data_mut(|d| d.get_temp::<bool>(net_custom_id)).unwrap_or(false);
    let mut mask_custom: bool = ui.data_mut(|d| d.get_temp::<bool>(mask_custom_id)).unwrap_or(!preset_mask(prefix0));
    let mut base_text: String = ui.data_mut(|d| d.get_temp::<String>(base_buf_id)).unwrap_or_else(|| base0.to_string());

    let mut prefix = prefix0;
    let fa = [
        netplan::first_free(netplan::PrivateBlock::C, prefix, host),
        netplan::first_free(netplan::PrivateBlock::B, prefix, host),
        netplan::first_free(netplan::PrivateBlock::A, prefix, host),
    ];

    // Re-sync the controls if the stored subnet changed outside this code.
    let cur = cfg.nat_subnet.clone().unwrap_or_default();
    let last: String = ui.data_mut(|d| d.get_temp::<String>(last_id)).unwrap_or_default();
    if cur != last {
        net_custom = !fa.contains(&base0);
        mask_custom = !preset_mask(prefix0);
        base_text = base0.to_string();
    }
    let mut base = if net_custom { base_text.parse().unwrap_or(base0) } else { base0 };

    let mut changed = false;     // any subnet field changed → recompose + mark dirty
    let mut committed = false;   // a deliberate commit → eligible for the override modal

    Grid::new("nat_grid").num_columns(2).striped(true).show(ui, |ui| {
        ui.label("Network");
        ui.horizontal(|ui| {
            let sel = if net_custom { "Custom".to_string() } else { base.to_string() };
            ComboBox::from_id_salt("net_preset").selected_text(sel).show_ui(ui, |ui| {
                for addr in fa {
                    if ui.selectable_label(!net_custom && base == addr, addr.to_string()).clicked() {
                        base = addr; net_custom = false; changed = true; committed = true;
                    }
                }
                if ui.selectable_label(net_custom, "Custom").clicked() {
                    net_custom = true;
                    base_text = base.to_string(); // seed the field with the current address
                }
            });
            if net_custom {
                let resp = ui.add(TextEdit::singleline(&mut base_text)
                    .hint_text("192.168.0.0").desired_width(130.0));
                if resp.changed() {
                    if let Ok(ip) = base_text.parse::<std::net::Ipv4Addr>() { base = ip; changed = true; }
                }
                if resp.lost_focus() { committed = true; }
            }
            // A custom mask shows the prefix right beside the base address.
            if mask_custom {
                ui.label("/");
                let mut bits = prefix.clamp(8, netplan::MAX_PREFIX);
                if ui.add(DragValue::new(&mut bits).range(8..=netplan::MAX_PREFIX)).changed() {
                    prefix = bits; changed = true; committed = true;
                }
            }
        });
        ui.end_row();

        ui.label("Subnet mask");
        let sel = if mask_custom { format!("Custom  (/{prefix})") } else { netplan::mask_label(prefix) };
        ComboBox::from_id_salt("net_mask").selected_text(sel).show_ui(ui, |ui| {
            for &p in netplan::MASK_PRESETS {
                if ui.selectable_label(!mask_custom && prefix == p, netplan::mask_label(p)).clicked() {
                    prefix = p; mask_custom = false; changed = true; committed = true;
                }
            }
            if ui.selectable_label(mask_custom, "Custom").clicked() {
                mask_custom = true;
                if preset_mask(prefix) { prefix = 24; } // default a fresh custom mask to /24
                changed = true;
            }
        });
        ui.end_row();
    });

    if changed {
        cfg.nat_subnet = Some(netplan::to_cidr(base, prefix));
        out.changed = true;
    }
    ui.data_mut(|d| {
        d.insert_temp(net_custom_id, net_custom);
        d.insert_temp(mask_custom_id, mask_custom);
        d.insert_temp(base_buf_id, base_text);
        d.insert_temp(last_id, cfg.nat_subnet.clone().unwrap_or_default());
    });

    // Derived addressing + sanity, from the live (unsnapped) base + prefix so the
    // snap note stays consistent across frames.
    let assess = netplan::classify(base, prefix, host);
    if let Some(msg) = &assess.hard_error {
        ui.label(RichText::new(format!("Invalid: {msg}")).color(Color32::from_rgb(0xd9, 0x4a, 0x3d)));
    } else if let Some(d) = &assess.derived {
        ui.label(RichText::new(format!(
            "Gateway (IRIS host) {}, Indy ec0 {}, {} usable hosts, broadcast {}",
            d.gateway, d.client, netplan::commas(d.usable_hosts), d.broadcast)).weak());
        if let Some(typed) = assess.off_boundary {
            ui.label(RichText::new(format!(
                "{typed} is not a network address; using {}/{}.", d.network, d.prefix)).weak());
        }
        match &assess.soft {
            Some(w) => {
                ui.horizontal(|ui| {
                    let sug = netplan::to_cidr(w.suggestion_net, w.suggestion_prefix);
                    ui.label(RichText::new(format!("Warning: {}", w.reason))
                        .color(Color32::from_rgb(0xd9, 0x9a, 0x3d)));
                    if ui.button(format!("Use {sug}")).clicked() {
                        cfg.nat_subnet = Some(sug); // external-change re-sync repaints the controls
                        out.changed = true;
                    }
                });
            }
            None => {
                ui.label(RichText::new("OK: private range, no conflict with your host networks")
                    .color(Color32::from_rgb(0x35, 0xb8, 0x4a)));
            }
        }
        // Pop the override modal when a deliberate edit (preset/mask pick, custom
        // mask bits, or the base field losing focus) lands on a soft-invalid
        // subnet. Live keystrokes don't trigger it — only a committed value does.
        if committed && assess.soft.is_some() {
            let w = assess.soft.as_ref().unwrap();
            out.prompt = Some(NetSanityPrompt {
                reason: w.reason.clone(),
                suggestion: netplan::to_cidr(w.suggestion_net, w.suggestion_prefix),
                revert_to: before.clone(),
            });
        }
    }

    ui.separator();
    ui.strong("Port forwards");
    let mut drop: Option<usize> = None;
    for (i, pf) in cfg.port_forward.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            ComboBox::from_id_salt(("proto", i))
                .selected_text(match pf.proto { ForwardProto::Tcp => "tcp", ForwardProto::Udp => "udp" })
                .show_ui(ui, |ui| {
                    out.changed |= ui.selectable_value(&mut pf.proto, ForwardProto::Tcp, "tcp").changed();
                    out.changed |= ui.selectable_value(&mut pf.proto, ForwardProto::Udp, "udp").changed();
                });
            ui.label("host");
            out.changed |= ui.add(DragValue::new(&mut pf.host_port).range(1..=65535)).changed();
            ui.label("to guest");
            out.changed |= ui.add(DragValue::new(&mut pf.guest_port).range(1..=65535)).changed();
            ComboBox::from_id_salt(("bind", i))
                .selected_text(match pf.bind { ForwardBind::Localhost => "localhost", ForwardBind::Any => "any" })
                .show_ui(ui, |ui| {
                    out.changed |= ui.selectable_value(&mut pf.bind, ForwardBind::Localhost, "localhost").changed();
                    out.changed |= ui.selectable_value(&mut pf.bind, ForwardBind::Any, "any").changed();
                });
            if ui.button("Remove").clicked() { drop = Some(i); }
        });
    }
    if let Some(i) = drop { cfg.port_forward.remove(i); out.changed = true; }

    let has_port = |p: u16| cfg.port_forward.iter().any(|f| f.guest_port == p);
    let mut add: Option<PortForwardConfig> = None;
    ui.menu_button("+ Add forward", |ui| {
        if ui.add_enabled(!has_port(23), egui::Button::new("Telnet (host 2323 to guest 23)"))
            .on_hover_text("Log in with: telnet localhost 2323. Needs the guest on IRIS's NAT subnet with telnetd running.")
            .clicked()
        {
            add = Some(PortForwardConfig { proto: ForwardProto::Tcp, host_port: 2323, guest_port: 23, bind: ForwardBind::Localhost });
            ui.close_menu();
        }
        if ui.add_enabled(!has_port(21), egui::Button::new("FTP (host 2121 to guest 21)"))
            .on_hover_text("Reach the guest's FTP server. Forwards the control port; file transfer also needs the data channel (see docs).")
            .clicked()
        {
            add = Some(PortForwardConfig { proto: ForwardProto::Tcp, host_port: 2121, guest_port: 21, bind: ForwardBind::Localhost });
            ui.close_menu();
        }
        if ui.button("Custom (empty row)").clicked() {
            add = Some(PortForwardConfig { proto: ForwardProto::Tcp, host_port: 0, guest_port: 0, bind: ForwardBind::Localhost });
            ui.close_menu();
        }
    });
    if let Some(pf) = add { cfg.port_forward.push(pf); out.changed = true; }

    ui.label(RichText::new(
        "A port forward maps a port on your computer to a port on the Indy, so host tools can reach \
         guest services (log in, copy files, and so on). Inbound only, and it works once the guest is \
         up on the NAT subnet. None exist by default.")
        .weak());

    ui.separator();
    ui.strong("NFS share");
    ui.label(RichText::new(
        "The Indy speaks NFS natively: the easiest, batteries-included way to move files between your \
         computer and the emulated machine. IRIS runs the NFS server for you, backed by the folder you \
         pick below; there's nothing to install and no NFS know-how required.")
        .weak());
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
        out.changed = true;
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
            out.changed |= ui.add(DragValue::new(&mut nfs.nfs_host_port).range(1..=65535)).changed();
            ui.end_row();
            ui.label("mountd host port");
            out.changed |= ui.add(DragValue::new(&mut nfs.mountd_host_port).range(1..=65535)).changed();
            ui.end_row();
        });
        // Live mount command — gateway + folder fill in to match the subnet.
        let gw = assess.derived.as_ref().map(|d| d.gateway.to_string()).unwrap_or_else(|| "192.168.0.1".into());
        let dir = if nfs.shared_dir.is_empty() { "/path/to/share".to_string() } else { nfs.shared_dir.clone() };
        ui.label(RichText::new("Pick a folder, boot the Indy, then mount it:").weak());
        ui.code(format!("mkdir /shared\nmount {gw}:{dir} /shared"));
        ui.label(RichText::new("Your files then appear at /shared on the Indy.").weak());
    }

    out
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

