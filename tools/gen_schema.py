"""Generate the IW3 zone schema used by the generic asset loader.

Inputs (from OpenAssetTools, used as format documentation):
  IW3_Assets.h           struct layouts
  XAssets/*.txt          serialisation rules ("ZoneCode")

Output: crates/iw3/src/zone/schema.json

Usage: python tools/gen_schema.py <dir with IW3_Assets.h and the .txt files>
       python tools/gen_schema.py <dir> <out.json> T5_Assets.h   (Black Ops: crates/t5)
       python tools/gen_schema.py <dir> <out.json> T4_Assets.h   (World at War: crates/t4)
"""

import json
import os
import re
import sys

PRIMS = {
    "char": (1, 1), "uchar": (1, 1), "bool": (1, 1), "int8_t": (1, 1), "uint8_t": (1, 1),
    "short": (2, 2), "ushort": (2, 2), "int16_t": (2, 2), "uint16_t": (2, 2),
    "int": (4, 4), "uint": (4, 4), "int32_t": (4, 4), "uint32_t": (4, 4), "long": (4, 4), "float": (4, 4),
    "double": (8, 8), "int64_t": (8, 8), "uint64_t": (8, 8), "__int64": (8, 8),
    "void": (1, 1),
}


def tokenize(src):
    src = re.sub(r"/\*.*?\*/", " ", src, flags=re.S)
    src = re.sub(r"//[^\n]*", " ", src)
    src = re.sub(r"^\s*#[^\n]*", " ", src, flags=re.M)
    return re.findall(r"[A-Za-z_][A-Za-z0-9_]*|0x[0-9A-Fa-f]+|\d+u?|::|\.\.\.|[{}()\[\];:,*=<>&|+\-/~.!]", src)


class Parser:
    def __init__(self, toks):
        self.t = toks
        self.i = 0
        self.types = {}      # name -> dict
        self.enums = {}      # constant -> value
        self.anon = 0

    def peek(self, k=0):
        return self.t[self.i + k] if self.i + k < len(self.t) else None

    def next(self):
        tok = self.t[self.i]
        self.i += 1
        return tok

    def expect(self, tok):
        got = self.next()
        if got != tok:
            raise SyntaxError(f"expected {tok!r}, got {got!r} near {self.t[self.i-8:self.i+4]}")

    def skip_balanced(self, open_, close):
        depth = 0
        while True:
            tok = self.next()
            if tok == open_:
                depth += 1
            elif tok == close:
                depth -= 1
                if depth == 0:
                    return

    def parse_align_attr(self):
        """type_align(N) / tdef_align32(N) / gcc_align32(N) etc. Returns N or None."""
        tok = self.peek()
        if tok and re.match(r"(type|tdef|gcc)_align(32|64)?$", tok):
            self.next()
            self.expect("(")
            n = int(self.next(), 0)
            self.expect(")")
            return None if tok.endswith("64") else n
        return None

    def parse(self):
        while self.peek() is not None:
            tok = self.peek()
            if tok == "namespace":
                self.next(); self.next(); self.expect("{")
            elif tok == "}":
                self.next()
            elif tok in ("struct", "union"):
                self.parse_record(top=True)
            elif tok == "enum":
                self.parse_enum()
            elif tok == "typedef":
                self.parse_typedef()
            elif tok == "static_assert":
                self.next(); self.skip_balanced("(", ")"); self.expect(";")
            else:
                self.next()

    def parse_enum(self):
        self.expect("enum")
        name = self.next() if self.peek() not in ("{", ":") else None
        size = 4
        if self.peek() == ":":
            self.next()
            base = []
            while self.peek() != "{":
                base.append(self.next())
            size = PRIMS.get(base[-1], (4, 4))[0]
        if self.peek() == ";":
            self.next()
            return
        self.expect("{")
        val = 0
        while self.peek() != "}":
            cname = self.next()
            if self.peek() == "=":
                self.next()
                expr = []
                while self.peek() not in (",", "}"):
                    expr.append(self.next())
                val = self.eval_const(expr)
            self.enums[cname] = val
            val += 1
            if self.peek() == ",":
                self.next()
        self.expect("}")
        self.expect(";")
        if name:
            self.types[name] = {"kind": "prim", "size": size, "align": size, "enum": True}

    def eval_const(self, toks):
        out = []
        for t in toks:
            if re.match(r"0x[0-9A-Fa-f]+$", t):
                out.append(str(int(t, 16)))
            elif re.match(r"\d+u?$", t):
                out.append(t.rstrip("u"))
            elif re.match(r"[A-Za-z_]\w*$", t):
                out.append(str(self.enums.get(t, 0)))
            else:
                out.append(t)
        return int(eval(" ".join(out)))

    def parse_typedef(self):
        self.expect("typedef")
        align = self.parse_align_attr()
        toks = []
        while self.peek() != ";":
            toks.append(self.next())
        self.expect(";")
        dims = []
        while toks and toks[-1] == "]":
            dims.insert(0, int(toks[-2], 0))
            toks = toks[:-3]
        name = toks[-1]
        btype, ptr = self.base_type(toks[:-1])
        self.types[name] = {"kind": "typedef", "of": btype, "ptr": ptr, "arr": dims, "force_align": align}

    def base_type(self, toks):
        toks = [t for t in toks if t not in ("const", "volatile", "struct", "union", "enum")]
        ptr = toks.count("*")
        toks = [t for t in toks if t != "*"]
        if not toks:
            return "int", ptr
        if toks[-1] in ("char", "short", "int", "long", "__int64"):
            unsigned = "unsigned" in toks
            name = {"char": "char", "short": "short", "int": "int", "long": "int", "__int64": "__int64"}[toks[-1]]
            if unsigned and name != "__int64":
                name = "u" + name
            return name, ptr
        if toks == ["unsigned"]:
            return "uint", ptr
        return toks[-1], ptr

    def parse_record(self, top=False):
        kind = self.next()  # struct/union
        align = self.parse_align_attr()
        name = None
        if self.peek() not in ("{", ";"):
            name = self.next()
        if self.peek() == ";":  # forward declaration
            self.next()
            return name
        self.expect("{")
        members = []
        while self.peek() != "}":
            members.extend(self.parse_member())
        self.expect("}")
        if name is None:
            self.anon += 1
            name = f"__anon{self.anon}"
        self.types[name] = {"kind": kind, "members": members, "force_align": align}
        if top:
            self.expect(";")
        return name

    def parse_member(self):
        if self.peek() in ("struct", "union") and (self.peek(1) == "{" or (self.peek(2) == "{" and re.match(r"(type|tdef|gcc)_align", self.peek(1) or ""))):
            inner = self.parse_record()
            if self.peek() == ";":
                self.next()
                return [{"name": None, "type": inner, "ptr": 0, "arr": [], "anon": True}]
            mname = self.next()
            dims = []
            while self.peek() == "[":
                self.next(); dims.append(int(self.next(), 0)); self.expect("]")
            self.expect(";")
            return [{"name": mname, "type": inner, "ptr": 0, "arr": dims}]
        member_align = None
        toks = []
        while self.peek() != ";":
            a = self.parse_align_attr()
            if a is not None:
                member_align = a
                continue
            toks.append(self.next())
        self.expect(";")
        # Pointer to array: T (*name)[N]
        if "(" in toks:
            i = toks.index("(")
            base = toks[:i]
            inner = toks[i + 1:toks.index(")")]
            name = inner[-1]
            dims = [int(x, 0) for x in re.findall(r"\[(\w+)\]", " ".join(toks[toks.index(")") + 1:]).replace(" ", ""))] \
                if False else []
            rest = toks[toks.index(")") + 1:]
            j = 0
            while j < len(rest):
                if rest[j] == "[":
                    dims.append(int(rest[j + 1], 0)); j += 3
                else:
                    j += 1
            btype, _ = self.base_type(base)
            return [{"name": name, "type": btype, "ptr": inner.count("*"), "arr": [], "pointee_arr": dims, "align": member_align}]
        # Bitfield
        if ":" in toks:
            i = toks.index(":")
            btype, ptr = self.base_type(toks[:i - 1])
            return [{"name": toks[i - 1], "type": btype, "ptr": ptr, "arr": [], "bits": int(toks[i + 1], 0)}]
        out = []
        # Possibly several declarators separated by commas.
        groups = [[]]
        for t in toks:
            if t == ",":
                groups.append([])
            else:
                groups[-1].append(t)
        first = groups[0]
        # Split the base type from the first declarator.
        dims_start = first.index("[") if "[" in first else len(first)
        name = first[dims_start - 1]
        base = first[:dims_start - 1]
        lead_ptr = 0
        while base and base[-1] == "*":
            lead_ptr += 1
            base = base[:-1]
        btype, ptr0 = self.base_type(base)
        decls = [first[dims_start - 1 - 0:]] + groups[1:]
        for d in decls:
            ptr = lead_ptr + ptr0 if d is decls[0] else d.count("*")
            d = [x for x in d if x != "*"]
            nm = d[0]
            dims = []
            j = 1
            while j < len(d):
                if d[j] == "[":
                    dims.append(self.eval_const([d[j + 1]]) if not d[j + 1].isdigit() else int(d[j + 1], 0)); j += 3
                else:
                    j += 1
            out.append({"name": nm, "type": btype, "ptr": ptr, "arr": dims, "align": member_align})
        return out


def layout(types):
    """Compute size/align/offsets (32-bit, MSVC-style)."""
    done = {}

    def size_align(tname):
        if tname in PRIMS:
            return PRIMS[tname]
        t = types.get(tname)
        if t is None:
            raise KeyError(tname)
        if tname in done:
            return done[tname]
        if t["kind"] == "prim":
            done[tname] = (t["size"], t["align"])
        elif t["kind"] == "typedef":
            if t["ptr"]:
                s, a = 4, 4
            else:
                s, a = size_align(t["of"])
            n = 1
            for d in t["arr"]:
                n *= d
            s *= n
            if t.get("force_align"):
                a = t["force_align"]
                s = (s + a - 1) // a * a if t["arr"] == [] and False else s
            done[tname] = (s, a)
        else:
            done[tname] = layout_record(tname, t)
        return done[tname]

    def member_sa(m):
        if m["ptr"]:
            s, a = 4, 4
        else:
            s, a = size_align(m["type"])
        n = 1
        for d in m["arr"]:
            n *= d
        if m.get("align"):
            a = max(a, m["align"])
        return s * n, a

    def layout_record(tname, t):
        is_union = t["kind"] == "union"
        off = 0
        max_align = 1
        size = 0
        bit_unit = None  # (offset, unit_size, bits_used)
        for m in t["members"]:
            if "bits" in m:
                us, ua = size_align(m["type"])
                if bit_unit is None or bit_unit[1] != us or bit_unit[2] + m["bits"] > us * 8:
                    off = (off + ua - 1) // ua * ua
                    bit_unit = [off, us, 0]
                    off += us
                m["offset"] = bit_unit[0]
                m["bit_offset"] = bit_unit[2]
                bit_unit[2] += m["bits"]
                max_align = max(max_align, ua)
                size = max(size, off)
                continue
            bit_unit = None
            s, a = member_sa(m)
            max_align = max(max_align, a)
            if is_union:
                m["offset"] = 0
                size = max(size, s)
            else:
                off = (off + a - 1) // a * a
                m["offset"] = off
                off += s
                size = off
            m["size"] = s
        if t.get("force_align"):
            max_align = max(max_align, t["force_align"])
        size = (size + max_align - 1) // max_align * max_align
        return size, max_align

    for name in list(types):
        try:
            sa = size_align(name)
            types[name]["size"], types[name]["align"] = sa
        except KeyError as e:
            types[name]["error"] = f"unknown type {e}"
    return types


def parse_rules(directory):
    """Parse ZoneCode .txt files into {type: {"type": {...}, "members": {path: {...}}}}."""
    rules = {}
    assets = {}
    files = sorted(f for f in os.listdir(directory) if f.endswith(".txt"))
    for fname in files:
        text = open(os.path.join(directory, fname), encoding="utf8").read()
        text = re.sub(r"//[^\n]*", "", text)
        cur = None
        for stmt in text.split(";"):
            stmt = " ".join(stmt.split())
            if not stmt:
                continue
            if stmt.startswith("use "):
                cur = stmt[4:].strip()
                rules.setdefault(cur, {"type": {}, "members": {}})
                continue
            if stmt.startswith("asset "):
                _, sname, aname = stmt.split()
                assets[sname] = aname
                continue
            m = re.match(r"reorder\s*(\w*)\s*:(.*)", stmt)
            if m:
                target = m.group(1) or cur
                rules.setdefault(target, {"type": {}, "members": {}})["type"]["reorder"] = m.group(2).split()
                continue
            if not stmt.startswith("set "):
                continue
            words = stmt.split(" ", 3)
            cmd = words[1]
            # Member references may be "Type::path" or relative to `cur`.
            def resolve(ref):
                parts = ref.split("::")
                if parts[0] in TYPE_NAMES and len(parts) > 1:
                    return parts[0], "::".join(parts[1:])
                return cur, ref
            if cmd == "block":
                args = stmt.split()[2:]
                if len(args) == 1:
                    rules[cur]["type"]["block"] = args[0]
                else:
                    t, m = resolve(args[0])
                    rules.setdefault(t, {"type": {}, "members": {}})["members"].setdefault(m, {})["block"] = args[1]
            elif cmd in ("string", "reusable", "scriptstring"):
                t, m = resolve(stmt.split()[2])
                rules.setdefault(t, {"type": {}, "members": {}})["members"].setdefault(m, {})[cmd] = True
            elif cmd in ("count", "condition", "arraysize"):
                rest = stmt.split(" ", 2)[2]
                ref, expr = rest.split(" ", 1)
                t, m = resolve(ref)
                rules.setdefault(t, {"type": {}, "members": {}})["members"].setdefault(m, {})[cmd] = expr.strip()
            elif cmd == "allocalign":
                args = stmt.split()[2:]
                if args[0] in TYPE_NAMES and "::" not in args[0]:
                    rules.setdefault(args[0], {"type": {}, "members": {}})["type"]["allocalign"] = int(args[1])
                else:
                    t, m = resolve(args[0])
                    rules.setdefault(t, {"type": {}, "members": {}})["members"].setdefault(m, {})["allocalign"] = args[1]
            elif cmd == "assetref":
                args = stmt.split()[2:]
                t, m = resolve(args[0])
                rules.setdefault(t, {"type": {}, "members": {}})["members"].setdefault(m, {})["assetref"] = args[1]
            elif cmd == "action":
                pass
            else:
                print("unknown command", stmt, file=sys.stderr)
    return rules, assets


TYPE_NAMES = set()


def main():
    src_dir = sys.argv[1]
    out = sys.argv[2] if len(sys.argv) > 2 else os.path.join(os.path.dirname(__file__), "..", "crates", "iw3", "src", "zone", "schema.json")
    header_name = sys.argv[3] if len(sys.argv) > 3 else "IW3_Assets.h"
    header = open(os.path.join(src_dir, header_name), encoding="utf8").read()
    p = Parser(tokenize(header))
    p.parse()
    types = layout(p.types)
    TYPE_NAMES.update(types)
    rules, assets = parse_rules(src_dir)
    errors = {k: v["error"] for k, v in types.items() if "error" in v}
    if errors:
        print("layout errors:", errors, file=sys.stderr)
    json.dump({"types": types, "enums": p.enums, "rules": rules, "assets": assets}, open(out, "w"), indent=0, sort_keys=True)
    # Sanity checks against sizes verified by hand.
    expect = {
        "XModel": 220, "Material": 80, "GfxWorld": 732, "clipMap_t": 284, "GfxImage": 36, "MaterialTechniqueSet": 148,
        "XSurface": 56, "cbrush_t": 80, "cLeafBrushNode_s": 20, "MaterialPass": 20, "GfxStaticModelDrawInst": 76,
        "GfxStateBitsLoadBits": 8, "MaterialTextureDefSamplerState": 1, "MaterialVertexDeclaration": 100,
        "GfxSurface": 48, "GfxCell": 56, "GfxPortal": 68, "DynEntityDef": 96, "ComPrimaryLight": 68,
    }
    if header_name != "IW3_Assets.h":
        expect = {}
    bad = {k: (types[k]["size"], v) for k, v in expect.items() if types.get(k, {}).get("size") != v}
    print(f"{len(types)} types, {len(rules)} rule sets, {len(assets)} assets; size mismatches: {bad}")


if __name__ == "__main__":
    main()
