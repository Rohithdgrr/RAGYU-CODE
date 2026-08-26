//! `/project` — view and manage persistent project memory
//! (`.govinda_project.json`): last scanned commit, preferred test and build
//! commands. See [`crate::project`].

use super::{dim, err, info, ok};

/// `/project [sub]`
///   (no args)          show current memory
///   set test <cmd…>    store the preferred test command
///   set build <cmd…>   store the preferred build/check command
///   clear test|build   remove a stored command
pub(super) fn handle(rest: &str) {
    let rest = rest.trim();
    let mut words = rest.split_whitespace();
    match words.next() {
        None | Some("show") => show(),
        Some("set") => match words.next() {
            Some("test") => set_command("test", words.collect::<Vec<_>>().join(" ")),
            Some("build") => set_command("build", words.collect::<Vec<_>>().join(" ")),
            other => err(format!(
                "unknown slot '{:?}' — use 'set test <cmd>' or 'set build <cmd>'",
                other.unwrap_or("")
            )),
        },
        Some("clear") => match words.next() {
            Some(slot @ ("test" | "build")) => clear_command(slot),
            other => err(format!(
                "unknown slot '{other:?}' — use 'clear test' or 'clear build'"
            )),
        },
        Some(other) => err(format!(
            "unknown subcommand '{other}' — try /project, /project set test <cmd>, /project set \
             build <cmd>, /project clear test|build"
        )),
    }
}

fn show() {
    let mem = crate::project::load();
    info("project memory (.govinda_project.json)");
    match &mem.last_scan_commit {
        Some(hash) => dim(format!("last scanned commit: {hash}")),
        None => dim("last scanned commit: (never scanned — run /scan)"),
    }
    print_slot("test", mem.test_command.as_deref());
    print_slot("build", mem.build_command.as_deref());
}

fn print_slot(name: &str, value: Option<&str>) {
    match value {
        Some(cmd) => ok(format!("{name} command: {cmd}")),
        None => dim(format!("{name} command: (auto-detected)")),
    }
}

fn set_command(slot: &str, command: String) {
    if command.trim().is_empty() {
        err("command must not be empty");
        return;
    }
    if command.chars().count() > 500 {
        err("command too long (cap 500 chars)");
        return;
    }
    let mut mem = crate::project::load();
    if slot == "test" {
        mem.test_command = Some(command.clone());
    } else {
        mem.build_command = Some(command.clone());
    }
    match crate::project::save_to(&std::env::current_dir().unwrap_or_default(), &mem) {
        Ok(()) => ok(format!("{slot} command saved: {command}")),
        Err(e) => err(format!("cannot save project memory: {e:#}")),
    }
}

fn clear_command(slot: &str) {
    let mut mem = crate::project::load();
    match slot {
        "test" => mem.test_command = None,
        "build" => mem.build_command = None,
        other => {
            err(format!("unknown slot '{other}'"));
            return;
        }
    }
    match crate::project::save_to(&std::env::current_dir().unwrap_or_default(), &mem) {
        Ok(()) => ok(format!("{slot} command cleared; auto-detection restored.")),
        Err(e) => err(format!("cannot save project memory: {e:#}")),
    }
}
