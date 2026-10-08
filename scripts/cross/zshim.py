"""Wrap `zig cc/c++/ar` for cargo cross-builds: drop the clang-style --target flag cc-rs adds."""
import subprocess
import sys

mode = sys.argv[1]
args = [a for a in sys.argv[2:] if not a.startswith("--target=")]
cmd = [sys.executable, "-m", "ziglang", mode]
if mode in ("cc", "c++"):
    cmd += ["-target", "x86_64-linux-gnu.2.28"]
sys.exit(subprocess.call(cmd + args))
