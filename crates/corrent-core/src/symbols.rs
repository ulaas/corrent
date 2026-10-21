//! Backtraces that can be resolved later (STABILITY.md O2). Shipped builds have no symbols next
//! to them, so a crash report lists every frame as an offset into the executable
//! (`corrent.exe+0x1a2b3c`) together with the image's identity. `--symbolize <report>`, run by
//! the same build with its `.pdb` beside it (`scripts/symbolize.ps1`), turns those offsets back
//! into functions and lines.

use std::ffi::c_void;
use std::fmt::Write as _;

/// The running executable as it was mapped: offsets are taken against `base`; `size` and
/// `stamp` tell builds apart (the PE link time on Windows, the file's length elsewhere).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub name: String,
    pub base: usize,
    pub size: usize,
    pub stamp: u64,
}

impl Image {
    fn header(&self) -> String {
        format!("image: {} base=0x{:x} size=0x{:x} stamp=0x{:x}", self.name, self.base, self.size, self.stamp)
    }

    fn parse(line: &str) -> Option<Image> {
        let rest = line.strip_prefix("image: ")?;
        // From the right: the name may contain spaces (`corrent (1).exe`).
        let mut parts = rest.rsplitn(4, ' ');
        let mut field = |key: &str| -> Option<u64> {
            let v = parts.next()?.strip_prefix(key)?.strip_prefix("=0x")?;
            u64::from_str_radix(v, 16).ok()
        };
        let stamp = field("stamp")?;
        let size = field("size")? as usize;
        let base = field("base")? as usize;
        let name = parts.next()?.to_string();
        Some(Image { name, base, size, stamp })
    }

    fn offset(&self, ip: usize) -> Option<usize> {
        (ip >= self.base && ip - self.base < self.size).then(|| ip - self.base)
    }
}

/// The running executable, where that is known (Windows and Linux).
pub fn this_image() -> Option<Image> {
    let name = std::env::current_exe().ok()?.file_name()?.to_string_lossy().into_owned();
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetModuleHandleW(name: *const u16) -> *const u8;
        }
        // SAFETY: a null name returns the executable's own module, mapped for the whole run;
        // its PE headers are readable at the offsets the format fixes.
        unsafe {
            let base = GetModuleHandleW(std::ptr::null());
            if base.is_null() {
                return None;
            }
            let nt = base.add(base.add(0x3c).cast::<u32>().read_unaligned() as usize);
            // FileHeader.TimeDateStamp, OptionalHeader.SizeOfImage.
            let stamp = nt.add(8).cast::<u32>().read_unaligned();
            let size = nt.add(24 + 56).cast::<u32>().read_unaligned();
            Some(Image { name, base: base as usize, size: size as usize, stamp: u64::from(stamp) })
        }
    }
    #[cfg(target_os = "linux")]
    {
        let exe = std::fs::canonicalize("/proc/self/exe").ok()?;
        let stamp = std::fs::metadata(&exe).ok()?.len();
        let maps = std::fs::read_to_string("/proc/self/maps").ok()?;
        let (mut lo, mut hi) = (usize::MAX, 0usize);
        for l in maps.lines().filter(|l| l.ends_with(&*exe.to_string_lossy())) {
            let range = l.split(' ').next()?;
            let (a, b) = range.split_once('-')?;
            lo = lo.min(usize::from_str_radix(a, 16).ok()?);
            hi = hi.max(usize::from_str_radix(b, 16).ok()?);
        }
        (lo < hi).then(|| Image { name, base: lo, size: hi - lo, stamp })
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = name;
        None
    }
}

fn describe(out: &mut String, name: Option<String>, file: Option<&std::path::Path>, line: Option<u32>) {
    let _ = write!(out, "  {}", name.unwrap_or_else(|| "<unknown>".into()));
    if let (Some(f), Some(l)) = (file, line) {
        let _ = write!(out, "\n        at {}:{l}", f.display());
    }
}

/// This thread's backtrace for a report: the image line, then one frame per line as an offset
/// into the image (absolute for other modules), with whatever this process can resolve itself.
pub fn backtrace_text() -> String {
    let image = this_image();
    let bt = backtrace::Backtrace::new();
    let mut out = String::new();
    if let Some(i) = &image {
        let _ = writeln!(out, "{}", i.header());
    }
    for (n, frame) in bt.frames().iter().enumerate() {
        let ip = frame.ip() as usize;
        let _ = write!(out, "{n:>4}: ");
        match image.as_ref().and_then(|i| Some((i, i.offset(ip)?))) {
            Some((i, off)) => _ = write!(out, "{}+0x{off:x}", i.name),
            None => _ = write!(out, "0x{ip:x}"),
        }
        match frame.symbols() {
            [] => describe(&mut out, None, None, None),
            symbols => {
                for (k, s) in symbols.iter().enumerate() {
                    if k > 0 {
                        out.push_str("\n      ");
                    }
                    describe(&mut out, s.name().map(|n| n.to_string()), s.filename(), s.lineno());
                }
            }
        }
        out.push('\n');
    }
    out
}

/// `--symbolize <report>`: prints the resolved report, or why not (exit code 1).
pub fn cli(report: &std::path::Path) -> i32 {
    match std::fs::read_to_string(report).map_err(|e| e.to_string()).and_then(|t| symbolize(&t)) {
        Ok(text) => {
            print!("{text}");
            0
        }
        Err(e) => {
            eprintln!("{}: {e}", report.display());
            1
        }
    }
}

/// A report's frames resolved by this build: every `<image>+0x<offset>` frame line is replaced
/// by the functions and lines at that offset; everything else is kept. Refuses a report from
/// another build (the offsets would point at other code).
pub fn symbolize(report: &str) -> Result<String, String> {
    let theirs = report.lines().find_map(Image::parse).ok_or("the report lists no image (an older report)")?;
    let ours = this_image().ok_or("this platform can't resolve reports")?;
    symbolize_with(report, &theirs, &ours, |addr, out| {
        let mut found = false;
        // The frame's address is where the call returns to; one byte back is the call itself.
        backtrace::resolve(addr.saturating_sub(1) as *mut c_void, |s| {
            if found {
                out.push_str("\n      ");
            }
            found = true;
            describe(out, s.name().map(|n| n.to_string()), s.filename(), s.lineno());
        });
        found
    })
}

fn symbolize_with(
    report: &str,
    theirs: &Image,
    ours: &Image,
    mut resolve: impl FnMut(usize, &mut String) -> bool,
) -> Result<String, String> {
    // Size and link stamp identify the build (the user may have renamed the exe).
    if theirs.size != ours.size || theirs.stamp != ours.stamp {
        // English, like the rest of the command line (and the report).
        let mut why = String::new();
        let _ = write!(
            why,
            "the report is from another build of {} (stamp 0x{:x}, this one is {} 0x{:x})",
            theirs.name, theirs.stamp, ours.name, ours.stamp
        );
        return Err(why);
    }
    let prefix = format!("{}+0x", theirs.name);
    let mut out = String::new();
    let mut lines = report.lines().peekable();
    while let Some(line) = lines.next() {
        let frame = line.split_once(": ").and_then(|(n, rest)| {
            let n = n.trim();
            let off = rest.strip_prefix(&prefix)?;
            let hex = off.split(|c: char| !c.is_ascii_hexdigit()).next()?;
            Some((n, usize::from_str_radix(hex, 16).ok()?))
        });
        let Some((n, off)) = frame.filter(|(n, _)| n.parse::<u32>().is_ok()) else {
            out.push_str(line);
            out.push('\n');
            continue;
        };
        // The report's own names and lines for this frame (indented continuation lines).
        while lines.peek().is_some_and(|l| l.starts_with("      ")) {
            lines.next();
        }
        let _ = write!(out, "{n:>4}: {}+0x{off:x}", theirs.name);
        if !resolve(ours.base + off, &mut out) {
            out.push_str("  <not found in the symbols>");
        }
        out.push('\n');
    }
    Ok(out)
}

#[inline(never)]
fn a_frame_to_find() -> String {
    std::hint::black_box(backtrace_text())
}

/// A report as a shipped build writes it (no symbols beside the exe): offsets only.
fn without_names(text: &str) -> String {
    text.lines()
        .filter(|l| !l.starts_with("      "))
        .map(|l| match l.split_once(": ") {
            Some((n, rest)) if n.trim().parse::<u32>().is_ok() => {
                format!("{n}: {}", rest.split_once("  ").map_or(rest, |(at, _)| at))
            }
            _ => l.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Writes a backtrace without names, resolves it again and checks that a known frame comes back
/// with its file and line: the profile's symbols are usable (`examples/symbols_check.rs`, run
/// with `--release`). Returns the resolved text, or the text that failed.
#[doc(hidden)]
pub fn self_check() -> Result<String, String> {
    let image = this_image().ok_or("no image on this platform")?;
    let text = a_frame_to_find();
    if !text.starts_with(&image.header()) || !text.contains(&format!("{}+0x", image.name)) {
        return Err(text);
    }
    let bare = without_names(&text);
    if bare.contains("a_frame_to_find") {
        return Err(bare);
    }
    let report = ["Corrent crash report\n\nbacktrace:\n", &bare, "\n\nrecent log:\nx y"].concat();
    let resolved = symbolize(&report)?;
    let ok = resolved.contains("a_frame_to_find")
        && resolved.contains("symbols.rs:")
        && resolved.ends_with("recent log:\nx y\n");
    if ok { Ok(resolved) } else { Err(resolved) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbolize_resolves_this_binary() {
        if this_image().is_some() {
            self_check().unwrap();
        }
    }

    #[test]
    fn reports_from_other_builds_are_refused_and_other_lines_kept() {
        let ours = Image { name: "corrent.exe".into(), base: 0x1000_0000, size: 0x5000, stamp: 7 };
        let theirs = Image { base: 0x2000_0000, ..ours.clone() };
        assert_eq!(Image::parse(&theirs.header()), Some(theirs.clone()));
        assert_eq!(Image::parse("image: x base=0x1 size=zz stamp=0x1"), None);
        let spaced = Image { name: "corrent (1).exe".into(), ..ours.clone() };
        assert_eq!(Image::parse(&spaced.header()), Some(spaced.clone()));
        assert!(symbolize_with("x", &spaced, &ours, |_, _| true).is_ok(), "a renamed exe is the same build");
        let report = format!(
            "{}\n   0: corrent.exe+0x10  old::name\n        at a.rs:1\n   1: 0x7ffa0000  ntdll\n   2: corrent.exe+0x20\n",
            theirs.header()
        );
        let mut asked = Vec::new();
        let out = symbolize_with(&report, &theirs, &ours, |addr, out| {
            asked.push(addr);
            out.push_str("  new::name");
            addr != ours.base + 0x20
        })
        .unwrap();
        assert_eq!(asked, vec![ours.base + 0x10, ours.base + 0x20], "rebased to this run");
        assert!(out.contains("   0: corrent.exe+0x10  new::name\n   1: 0x7ffa0000  ntdll\n"), "{out}");
        assert!(!out.contains("old::name") && !out.contains("a.rs"), "{out}");
        assert!(out.contains("   2: corrent.exe+0x20  new::name  <not found in the symbols>"), "{out}");
        let other = Image { stamp: 8, ..ours.clone() };
        assert!(symbolize_with(&report, &other, &ours, |_, _| true).is_err());
        assert!(symbolize("no image line").is_err());
    }
}
