"""Print the ELF class and DT_NEEDED entries of a 64-bit little-endian ELF binary."""
import struct
import sys

data = open(sys.argv[1], "rb").read()
assert data[:4] == b"\x7fELF", "not ELF"
print("ELF64" if data[4] == 2 else "ELF32", "machine", struct.unpack_from("<H", data, 18)[0])
e_shoff = struct.unpack_from("<Q", data, 0x28)[0]
e_shentsize, e_shnum, e_shstrndx = struct.unpack_from("<HHH", data, 0x3A)
sections = []
for i in range(e_shnum):
    off = e_shoff + i * e_shentsize
    name, typ, flags, addr, offset, size, link = struct.unpack_from("<IIQQQQI", data, off)
    sections.append((name, typ, offset, size, link))
dyn = [s for s in sections if s[1] == 6]  # SHT_DYNAMIC
if not dyn:
    print("static binary")
    sys.exit()
_, _, off, size, link = dyn[0]
strtab_off = sections[link][2]
needed = []
for i in range(0, size, 16):
    tag, val = struct.unpack_from("<qQ", data, off + i)
    if tag == 0:
        break
    if tag == 1:  # DT_NEEDED
        end = data.index(b"\0", strtab_off + val)
        needed.append(data[strtab_off + val:end].decode())
print("needed:", ", ".join(needed))
