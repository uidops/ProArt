# DDC/CI protocol notes

Transport and VCP notes gathered while building and testing ProArt (and its Python predecessor, ProArt Bar).
Everything here was verified on hardware (Apple M3 Pro, macOS 27, ASUS
ProArt PA278QV) unless stated otherwise.

## Apple Silicon DDC transport

* The classic public path `CGDisplayIOServicePort()` + `IOI2CSendRequest`
  on the `IOFramebuffer` I2C interface is **dead on Apple Silicon**
  (verified: the symbol is still exported but returns NULL on macOS 27).
* The working path (m1ddc, MonitorControl, DDCKit) is the **IOAVService**
  private-but-exported API family in the public IOKit framework, attached
  to `DCPAVServiceProxy` IORegistry nodes:
  * `IOAVServiceCreateWithService(0, service, &err)`
  * `IOAVServiceWriteI2C(av, 0x37, 0x51, buf, len)`
  * `IOAVServiceReadI2C(av, 0x37, 0x51, buf, len)`
  * 7-bit DDC address `0x37` (8-bit write form `0x6E` is rejected with
    `IOReturn 0xE0114000` on Apple Silicon), DDC register offset `0x51`,
    request bytes XOR-checksummed starting from `0x6E ^ 0x51`,
    the write request sent twice, ~50 ms settle delays.
  * `IOAVServiceCopyEDID()` maps each proxy to a CoreGraphics display id
    so proxy ↔ display association is reliable across hotplug.
* Other platforms: `ddcutil` subprocess on Linux, `dxva2.dll`
  (`Get/SetVCPFeature`) on Windows.

## VCP features

* 0x10 brightness, 0x12 contrast, 0x60 input source, 0x62 volume,
  0x8D mute, **0xD6 power mode** — each read as `(current, maximum)`.
* A read can block indefinitely on misbehaving displays/firmware, which is
  why reads are opt-in here and every DDC transaction runs on a worker
  thread with a timeout.
* Power feature is 0xD6 (reads `(1, 5)` = on, max 5), values 1=on … 4=off.
  An earlier note said 0xDC; that code answers with an unrelated vendor
  range and ignores writes, so anything targeting it is a no-op.

## Verification performed on real hardware

* Machine: Apple M3 Pro, macOS 27; display: ASUS PA278QV (ProArt).
* Probe scripts (logic reproduced in
  `crates/proart-core/src/backend/macos.rs`):
  * confirmed `CGDisplayIOServicePort` returns 0 (classic path dead),
  * enumerated `IOAVService*` symbols, attached to `DCPAVServiceProxy`,
  * read VCP 0x10 → 86/100, 0x12 → 100/100, 0x62 → 100, input 0x60 → 0x11,
  * write round-trip (set brightness to read value, re-read matched).
* End-to-end: `proart status`, `proart set --brightness 90`
  (read back 90, restored to 86), GUI icon bar rendered with all five
  default controls.

## ProArt OSD features (PA278QV scan)

A full read scan (opcodes 0x01-0xFF, no capabilities string available) found
these answering. Reads only unless noted.

| Opcode | Meaning | Reading |
|--------|---------|---------|
| 0x08 | restore factory colour defaults (MCCS) | answers; **never written** in testing |
| 0x0C / 0x0B | colour temperature request / increment | 35 / 100 (=6500 K) |
| 0x14 | colour temperature (ASUS ProArt list, MCCS opcode) | 5 (6500 K), max 12; write verified in DC modes 14/15/33, **ignored (locked) in mode 13** |
| 0x16/0x18/0x1A | RGB gain | 50/100; write + readback verified |
| 0x6C/0x6E/0x70 | RGB black level | 50/100; write + readback verified |
| 0x87 | sharpness (MCCS) | 0..100; write + readback verified |
| 0xC0 / 0xC9 / 0xDF / 0xAE | usage hours / firmware / MCCS version / refresh | 1485 h / 0.1 / 2.1 / 75.10 Hz |
| 0xCA | OSD / key lock | 1 (max 2) while buttons work -> 1=off, 2=on *inferred* |
| 0xCC | OSD language | 2 = English, max 13 (MCCS table) |
| 0xDC | vendor mode register | see below |
| 0xE6, 0xFD, 0x52 | vendor / volatile | 0, 98, changes on its own |

* **Sharpness is 0x87.** 0x20 (what an earlier build used) is horizontal
  position and does not answer on this monitor.
* **Mute (0x8D) is not implemented** by the PA278QV; ProArt Bar emulates mute
  with volume 0 and restores the previous level.
* **Not exposed over DDC/CI (no register answers):** saturation (0x8A), hue
  (0x90), gamma (0x72).
* **0xDC is the ProArt preset register on this model**, learned from the OSD
  with `proart learn preset` on a PA278QV:

  | Mode | 0xDC |
  |------|------|
  | Standard | 0 |
  | sRGB | 13 |
  | Rec. 709 | 33 |
  | DCI-P3 | 35 |
  | Scenery | 11 |
  | Reading | 23 |
  | Darkroom | 24 |
  | Rapid Rendering | 34 |

  This table ships as the built-in profile for `PA278QV` (other models still
  learn their own). It also explains earlier observations: the monitor's
  original mode 13 is **sRGB**, which locks colour temperature; Standard (0)
  reset brightness to 26 and contrast to 100; values 14 and 15 are accepted
  but are not among the eight OSD presets (unidentified). ASUS's ProArt caps
  string lists `DC(26 0D 27 28 25 24 21 20 0E 0F)`, a different model's set.
* **The monitor settles slowly.** Mode changes take seconds to apply; after a
  write, allow >= 4 s before reading back. (The first 0.7 s sweep's accepted
  set - 0, 11, 13, 14, 15, 23, 24, 33-35 - turned out to be right.)
* **Modes lock controls.** In mode 13 a colour-temperature write is silently
  ignored (the monitor answers nothing, and the value reads back unchanged);
  in modes 14/15/33 it works. `proart set` reads back after the
  settle time and exits 1 with a "locked in the current picture mode" warning.
* **0xE6 Blue Light Filter** answers `(0, 255)` and reads back the OSD level, but
  writes (levels 1-3, 4 s settle) are ignored in every mode tried, while the OSD
  itself changes the level fine. Blue Light is therefore not built in: it is
  learned per monitor (`proart learn blue_light`).

### Where the ASUS opcodes come from

ASUS publishes no VCP table, but two public sources describe how its Windows
app (**DisplayWidget Center**, a .NET WPF app) talks to monitors:

* ASUS's own CLI repo, `ASUS-Display/asus-display-control` (Apache-2.0,
  binaries + `cli/docs/CLI_REFERENCE.md`): property names with value tables
  (BlueLightFilter 0-max, Gamma 1.8-2.6, InputRange 0-4, ProArtPreset, ...).
* A community write-up of a decompile of DisplayWidget Center 1.3.0.4
  (`adalinmik/Asus-Displaywidget-Linux`, `docs/PROTOCOL.md` and `src/caps.rs`):
  the opcode map and the built-in ProArt capabilities strings.

Findings that shaped this app: the transport is plain DDC/CI through
`dxva2.dll` (`GetVCPFeatureAndVCPFeatureReply`, `SetVCPFeature`,
`CapabilitiesRequestAndCapabilitiesReply`) - no driver, USB or network
channel; the app sleeps 20 ms before each write and serialises all calls;
per-model support comes from the capabilities string, with built-in strings for
ProArt monitors that do not answer; ProArt presets live in 0xE3
(`preset<<8|0xFF`), the rest of the picture settings in 0xDC on older models.
Built-in opcodes from there (0xE0 trace free, 0xE1 ASCR /
power saving by table generation (0xEF), 0xE4 input range, 0xE7 QuickFit,
0x72 gamma, 0xE3 calibrated preset) are only used when the monitor answers a
read of them. The Windows `Splendid` table (1-8) does **not** apply to the
PA278QV (its accepted 0xDC values are 13/14/15/33).

The Windows "driver" for a PA278QV is a 15 KB generic monitor INF (EDID / ICC
only); the controls are in DisplayWidget Center. ASUS's CDN and its driver API
returned 403 to scripted requests, so the installer itself was not inspected;
everything above is from the two public sources and from hardware tests.

### Learning the remaining items

Aspect Control, Vivid Pixel, Sound Source, AutoDetection, Adaptive-Sync, OSD
Setup, DisplayPort Stream and Power Indicator have no published opcode or value
table, and other models' preset tables are not known. ProArt Bar ships their *names* and
learns the opcode and value of each name from the monitor: `proart learn preset` asks you to pick each mode in the monitor's
OSD, scans every register after each step and keeps the one that changes
consistently. The result is stored per display in `config.json`
(`displays.<key>.feature_map`). Until an item is learned nothing is ever
written to a vendor opcode.
