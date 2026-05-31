use crate::handle::Status;
use iris::config::MachineConfig;

/// Reasons it is risky to force-stop right now.
#[derive(Debug, Clone, Default)]
pub struct UnsafeReasons {
    pub irix_running: bool,
    pub dirty_cow: usize,
    /// SCSI IDs whose backing image is a .chd (writes lost unless overlay).
    pub chd_no_overlay: Vec<u8>,
}

impl UnsafeReasons {
    pub fn is_empty(&self) -> bool {
        !self.irix_running && self.dirty_cow == 0 && self.chd_no_overlay.is_empty()
    }
}

/// Evaluate whether stopping the emulator right now is safe.
/// Safe iff: PowerOff seen, OR sitting at PROM, OR no dirty COW sectors.
pub fn evaluate(status: &Status, cfg: &MachineConfig) -> UnsafeReasons {
    let mut r = UnsafeReasons::default();
    let safe = status.power_off_seen || status.in_prom || status.dirty_cow == 0;
    if safe {
        return r;
    }
    r.irix_running = !status.in_prom && !status.power_off_seen;
    r.dirty_cow = status.dirty_cow;
    for (id, dev) in &cfg.scsi {
        if dev.path.ends_with(".chd") && !dev.overlay {
            r.chd_no_overlay.push(*id);
        }
    }
    r.chd_no_overlay.sort();
    r
}

/// Human-readable lines for the confirmation dialog.
pub fn reason_lines(r: &UnsafeReasons) -> Vec<String> {
    let mut out = Vec::new();
    if r.irix_running {
        out.push("IRIX is running — force-stop may corrupt the filesystem.".into());
    }
    if r.dirty_cow > 0 {
        out.push(format!(
            "{} dirty COW overlay sector(s) have not been flushed to disk.",
            r.dirty_cow
        ));
    }
    for id in &r.chd_no_overlay {
        out.push(format!(
            "scsi{id} is a CHD image without overlay=true — writes will be discarded."
        ));
    }
    out
}
