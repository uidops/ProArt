//! `proart`: command line control for ASUS ProArt monitors.

use std::io::{self, BufRead, Write};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use clap::{Parser, Subcommand};
use proart_core::backend::default_backend;
use proart_core::features::{self, Feature, Group, Kind};
use proart_core::learn::{LearnSession, Outcome};
use proart_core::manager::{DisplayState, Manager, Status};
use proart_core::{config, Settings, VERSION};

#[derive(Parser)]
#[command(name = "proart", version = VERSION, about = "Control ASUS ProArt monitors over DDC/CI")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List detected displays
    List,
    /// Show the quick controls (brightness, contrast, volume, preset, input)
    Status {
        #[arg(long)]
        json: bool,
        #[arg(long)]
        display: Option<String>,
    },
    /// List every ProArt OSD feature and its state
    Features {
        #[arg(long)]
        display: Option<String>,
        /// Only one group: preset, blue, palette, image, sound, input, system
        #[arg(long)]
        group: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Read a feature, e.g. `proart get brightness`
    Get {
        feature: String,
        #[arg(long)]
        display: Option<String>,
    },
    /// Write a feature, e.g. `proart set brightness 60` or `proart set preset "sRGB Mode"`
    Set {
        feature: String,
        value: Option<String>,
        #[arg(long)]
        display: Option<String>,
        /// Apply to every display
        #[arg(long)]
        all: bool,
        /// Confirm features that are hard to undo (color_reset, key_lock, input)
        #[arg(long)]
        yes: bool,
        /// Seconds to let the monitor settle before reading back
        #[arg(long, default_value_t = 2.5)]
        wait: f64,
    },
    /// Learn an ASUS-specific OSD item by watching the monitor
    Learn {
        feature: String,
        #[arg(long)]
        display: Option<String>,
        /// Comma-separated values to learn (default: all)
        #[arg(long)]
        names: Option<String>,
        /// Drop the learned mapping / "not available" mark
        #[arg(long)]
        forget: bool,
        /// Mark the item as not available on this display
        #[arg(long)]
        skip: bool,
    },
    /// Model, firmware, usage hours and refresh rate
    Info {
        #[arg(long)]
        display: Option<String>,
    },
    /// Power a display on or off
    Power {
        state: PowerState,
        #[arg(long)]
        display: Option<String>,
        #[arg(long)]
        all: bool,
    },
    /// Toggle mute
    Mute {
        #[arg(long)]
        display: Option<String>,
    },
    /// Saved presets: list, save NAME, apply NAME, delete NAME
    Preset {
        action: PresetAction,
        name: Option<String>,
    },
    /// Show the config path, print it, or reset it
    Config {
        #[arg(long)]
        path: bool,
        #[arg(long)]
        reset: bool,
    },
}

#[derive(Clone, clap::ValueEnum)]
enum PowerState {
    On,
    Off,
}

#[derive(Clone, clap::ValueEnum)]
enum PresetAction {
    List,
    Save,
    Apply,
    Delete,
}

type Res<T> = Result<T, String>;

fn manager() -> Arc<Manager> {
    let mgr = Manager::new(default_backend(), Settings::load(), Arc::new(|_| {}));
    mgr.refresh();
    mgr
}

fn pick(mgr: &Manager, selector: &Option<String>) -> Res<Arc<DisplayState>> {
    if mgr.displays().is_empty() {
        return Err("no displays found".into());
    }
    mgr.find_display(selector.as_deref())
        .ok_or_else(|| format!("display not found: {}", selector.clone().unwrap_or_default()))
}

fn feature_arg(text: &str) -> Res<&'static Feature> {
    features::find(text).ok_or_else(|| format!("unknown feature: {text} (see `proart features`)"))
}

/// One status cell: the current value, or why there is none.
fn describe(mgr: &Manager, st: &DisplayState, f: &'static Feature) -> String {
    match mgr.status(st, f) {
        Status::Unlearned => return format!("not learned (proart learn {})", f.id),
        Status::Unsupported => {
            let marked = mgr.marked_unsupported(st, f);
            return if marked {
                format!("not exposed by this display (retry: proart learn {} --forget)", f.id)
            } else {
                "not supported by this display".into()
            };
        }
        Status::Ok => {}
    }
    if f.kind == Kind::Action {
        return "(action)".into();
    }
    match mgr.read(st, f) {
        None => "no reply".into(),
        Some(v) if f.kind == Kind::Choice => match mgr.choice_name(st, f, v) {
            Some(name) => format!("{v} ({name})"),
            None => format!("{v} (unnamed, 0x{v:02X})"),
        },
        Some(v) => v.to_string(),
    }
}

fn run(cli: Cli) -> Res<()> {
    match cli.command {
        Cmd::List => {
            let mgr = manager();
            for (i, st) in mgr.displays().iter().enumerate() {
                let info = &st.info;
                println!("{}. {}  key={}", i + 1, mgr.label(st), st.key());
                println!(
                    "   backend={}  builtin={}  res={}x{}  serial={}",
                    info.backend, info.builtin, info.width, info.height,
                    if info.serial.is_empty() { "-" } else { &info.serial }
                );
            }
            Ok(())
        }
        Cmd::Status { json, display } => {
            let mgr = manager();
            let st = pick(&mgr, &display)?;
            let quick = mgr.with_settings(|s| s.ui.quick_features.clone());
            let mut rows = Vec::new();
            for id in &quick {
                let Some(f) = features::get(id) else { continue };
                if mgr.visible(&st, f) {
                    rows.push((f, describe(&mgr, &st, f)));
                }
            }
            if json {
                let map: serde_json::Map<String, serde_json::Value> =
                    rows.iter().map(|(f, v)| (f.id.to_string(), serde_json::Value::String(v.clone()))).collect();
                println!("{}", serde_json::to_string_pretty(&map).unwrap());
            } else {
                println!("{} ({})", mgr.label(&st), st.key());
                for (f, v) in rows {
                    println!("  {:<14} {v}", f.label);
                }
            }
            Ok(())
        }
        Cmd::Features { display, group, json } => {
            let mgr = manager();
            let st = pick(&mgr, &display)?;
            mgr.probe(&st);
            let only = match &group {
                Some(g) => Some(Group::parse(g).ok_or_else(|| format!("unknown group: {g}"))?),
                None => None,
            };
            let mut out = serde_json::Map::new();
            if !json {
                println!("{} ({})", mgr.label(&st), st.key());
            }
            for g in Group::ALL {
                if only.is_some_and(|o| o != g) {
                    continue;
                }
                let list: Vec<_> = features::in_group(g).filter(|f| f.only_gen.is_none_or(|x| x == st.gen())).collect();
                if list.is_empty() {
                    continue;
                }
                if !json {
                    println!("\n{}", g.label());
                }
                for f in list {
                    let text = describe(&mgr, &st, f);
                    if json {
                        out.insert(
                            f.id.to_string(),
                            serde_json::json!({"group": g.label(), "label": f.label, "vendor": f.vendor(),
                                               "verified": f.verified, "state": text}),
                        );
                    } else {
                        let tag = if f.verified { "" } else if f.vendor() { "  [vendor]" } else { "" };
                        println!("  {:<18} {:<26} {text}{tag}", f.id, f.label);
                    }
                }
            }
            if json {
                println!("{}", serde_json::to_string_pretty(&out).unwrap());
            }
            Ok(())
        }
        Cmd::Get { feature, display } => {
            let mgr = manager();
            let st = pick(&mgr, &display)?;
            let f = feature_arg(&feature)?;
            mgr.detect_generation(&st);
            println!("{}", describe(&mgr, &st, f));
            Ok(())
        }
        Cmd::Set { feature, value, display, all, yes, wait } => {
            let mgr = manager();
            let f = feature_arg(&feature)?;
            let targets = if all {
                mgr.displays()
            } else {
                vec![pick(&mgr, &display)?]
            };
            let mut warn = false;
            for st in targets {
                mgr.detect_generation(&st);
                match mgr.status(&st, f) {
                    Status::Unsupported => return Err(format!("{} is not available on {}", f.label, mgr.label(&st))),
                    Status::Unlearned => {
                        return Err(format!(
                            "{} is ASUS-specific and has not been learned for this display yet: proart learn {}",
                            f.label, f.id
                        ))
                    }
                    Status::Ok => {}
                }
                let parsed = parse_value(&mgr, &st, f, value.as_deref())?;
                if f.dangerous && !yes && f.id != "input" {
                    return Err(format!("{} changes monitor state that is hard to undo; repeat with --yes", f.label));
                }
                let (sent, applied) = mgr.write_confirmed(&st, f, parsed, Duration::from_secs_f64(wait));
                if !sent {
                    return Err(format!("{}: {} rejected", mgr.label(&st), f.label));
                }
                println!("{}: {} = {}", mgr.label(&st), f.label, value.clone().unwrap_or_else(|| "done".into()));
                if applied == Some(false) {
                    eprintln!(
                        "warning: the monitor did not change - {} is probably locked in its current \
                         picture mode (change the ProArt Preset and retry)",
                        f.label
                    );
                    warn = true;
                }
            }
            mgr.save_settings();
            if warn { Err(String::new()) } else { Ok(()) }
        }
        Cmd::Learn { feature, display, names, forget, skip } => {
            let mgr = manager();
            let st = pick(&mgr, &display)?;
            let f = feature_arg(&feature)?;
            if !f.vendor() || f.documented {
                return Err(format!("{} is a standard or documented feature; nothing to learn", f.label));
            }
            if forget {
                mgr.forget(&st, f);
                println!("forgot learned mapping for {}", f.label);
                return Ok(());
            }
            if skip {
                mgr.mark_unsupported(&st, f);
                println!("{}: marked as not available on {}", f.label, mgr.label(&st));
                return Ok(());
            }
            learn(&mgr, &st, f, names)
        }
        Cmd::Info { display } => {
            let mgr = manager();
            let st = pick(&mgr, &display)?;
            println!("Model       {}", if st.info.name.is_empty() { "-" } else { &st.info.name });
            if !st.info.serial.is_empty() {
                println!("Serial      {}", st.info.serial);
            }
            if st.info.width > 0 {
                println!("Resolution  {}x{}", st.info.width, st.info.height);
            }
            for (label, text) in mgr.info_rows(&st) {
                println!("{label:<11} {text}");
            }
            Ok(())
        }
        Cmd::Power { state, display, all } => {
            let mgr = manager();
            let targets = if all { mgr.displays() } else { vec![pick(&mgr, &display)?] };
            let on = matches!(state, PowerState::On);
            for st in targets {
                if !mgr.power(&st, on) {
                    return Err(format!("{}: power {} rejected", mgr.label(&st), if on { "on" } else { "off" }));
                }
                println!("{}: power {}", mgr.label(&st), if on { "on" } else { "off" });
            }
            std::thread::sleep(Duration::from_secs(1));
            Ok(())
        }
        Cmd::Mute { display } => {
            let mgr = manager();
            let st = pick(&mgr, &display)?;
            mgr.toggle_mute(&st);
            println!("{}: mute toggled", mgr.label(&st));
            std::thread::sleep(Duration::from_millis(500));
            Ok(())
        }
        Cmd::Preset { action, name } => preset(action, name),
        Cmd::Config { path, reset } => {
            if path {
                println!("{}", config::config_path().display());
            } else if reset {
                Settings::default().save().map_err(|e| e.to_string())?;
                println!("config reset: {}", config::config_path().display());
            } else {
                println!("{}", serde_json::to_string_pretty(&Settings::load()).unwrap());
            }
            Ok(())
        }
    }
}

fn parse_value(mgr: &Manager, st: &DisplayState, f: &'static Feature, text: Option<&str>) -> Res<Option<u16>> {
    match f.kind {
        Kind::Action => Ok(None),
        Kind::Slider => {
            let t = text.ok_or_else(|| format!("{} needs a value", f.id))?;
            t.trim().parse::<u16>().map(Some).map_err(|_| format!("{} expects a number", f.id))
        }
        Kind::Choice => {
            let t = text.ok_or_else(|| format!("{} needs a value", f.id))?;
            let res = mgr.resolve(st, f);
            let choices: Vec<(&str, u16)> = res.choices.iter().filter_map(|(n, c)| c.map(|c| (n.as_str(), c))).collect();
            if let Some(code) = features::match_choice(choices.iter().copied(), t) {
                return Ok(Some(code));
            }
            if let Ok(n) = t.trim().parse::<u16>() {
                return Ok(Some(n));
            }
            let names = choices.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", ");
            Err(format!("unknown value for {}: {t}\nchoices: {}", f.id, if names.is_empty() { "(none learned)" } else { &names }))
        }
    }
}

fn ask(prompt: &str) -> Res<String> {
    print!("{prompt}");
    io::stdout().flush().ok();
    let mut line = String::new();
    let n = io::stdin().lock().read_line(&mut line).map_err(|e| e.to_string())?;
    if n == 0 {
        return Err("learn needs an interactive terminal".into());
    }
    Ok(line.trim().to_string())
}

fn learn(mgr: &Manager, st: &DisplayState, f: &'static Feature, names: Option<String>) -> Res<()> {
    let names: Vec<String> = if f.kind == Kind::Slider {
        vec!["minimum".into(), "maximum".into()]
    } else if let Some(n) = names {
        n.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
    } else {
        f.options.iter().map(|s| s.to_string()).collect()
    };
    println!("Learning {} on {}.", f.label, mgr.label(st));
    println!("Scanning the monitor's registers (about 2-3 minutes: every unanswered register costs a timeout) ...");
    let mut session = LearnSession::begin(mgr, st, f, names.clone());
    for name in &names {
        let answer = ask(&format!(
            "On the monitor, set {} to \u{201c}{name}\u{201d}, then press Enter (Enter without changing it, or s, skips this value): ",
            f.label
        ))?;
        if answer.to_lowercase().starts_with('s') {
            continue;
        }
        if !session.capture(mgr, st, name) {
            println!("  nothing changed - ignoring \u{201c}{name}\u{201d} (not in this monitor's menu?)");
        }
    }
    match session.outcome() {
        Outcome::NothingChanged => {
            mgr.mark_unsupported(st, f);
            println!(
                "Nothing changed in any step, so {} is not available on {}. Marked as not available (retry with: proart learn {} --forget).",
                f.label, mgr.label(st), f.id
            );
        }
        Outcome::NeedMore => {
            return Err("only one value changed - run it again and change at least two (start from a value different from the first one asked)".into())
        }
        Outcome::NoRegister => {
            mgr.mark_unsupported(st, f);
            println!(
                "No register changed consistently, so {} does not expose {} over DDC/CI. Marked as not available (retry with: proart learn {} --forget).",
                mgr.label(st), f.label, f.id
            );
        }
        Outcome::Found(found) => {
            let mut vcp = found[0];
            if found.len() > 1 {
                if f.vcp.is_some_and(|h| found.contains(&h)) {
                    vcp = f.vcp.unwrap();
                } else {
                    println!("Several registers matched: {}", found.iter().map(|c| format!("0x{c:02X}")).collect::<Vec<_>>().join(", "));
                    let pick = ask(&format!("Use which? [0x{:02X}]: ", found[0]))?;
                    if !pick.is_empty() {
                        vcp = u8::from_str_radix(pick.trim_start_matches("0x"), 16).map_err(|e| e.to_string())?;
                    }
                }
            }
            let values = session.values(vcp);
            mgr.save_learned(st, f, vcp, values.clone());
            println!(
                "Learned {}: VCP 0x{vcp:02X}  {}",
                f.label,
                values.iter().map(|(n, v)| format!("{n}={v}")).collect::<Vec<_>>().join(", ")
            );
        }
    }
    Ok(())
}

fn preset(action: PresetAction, name: Option<String>) -> Res<()> {
    let mgr = manager();
    if let PresetAction::List = action {
        for p in mgr.with_settings(|s| s.presets.clone()) {
            println!("{}  (power: {})", p.name, p.power);
        }
        return Ok(());
    }
    let name = name.ok_or("preset needs a name")?;
    match action {
        PresetAction::Save => {
            for st in mgr.displays() {
                mgr.probe(&st);
            }
            let p = mgr.capture_preset(&name, "star");
            mgr.with_settings(|s| s.presets.push(p));
            mgr.save_settings();
            println!("saved preset '{name}'");
        }
        PresetAction::Apply => {
            for st in mgr.displays() {
                mgr.detect_generation(&st);
            }
            let preset = mgr
                .with_settings(|s| s.presets.iter().find(|p| p.name.eq_ignore_ascii_case(&name) || p.id == name).cloned())
                .ok_or_else(|| format!("preset not found: {name} (try `proart preset list`)"))?;
            mgr.apply_preset(&preset);
            println!("applied preset '{}'", preset.name);
            std::thread::sleep(Duration::from_secs(1));
        }
        PresetAction::Delete => {
            mgr.with_settings(|s| s.presets.retain(|p| !p.name.eq_ignore_ascii_case(&name) && p.id != name));
            mgr.save_settings();
            println!("deleted preset {name}");
        }
        PresetAction::List => unreachable!(),
    }
    Ok(())
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            if !msg.is_empty() {
                eprintln!("{msg}");
            }
            ExitCode::FAILURE
        }
    }
}
