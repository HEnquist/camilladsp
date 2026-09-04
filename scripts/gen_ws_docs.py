#!/usr/bin/env python3
"""
Generate websocket API documentation from rustdoc JSON.

Writes the result directly to websocket.md in the repository root.

Usage:
    python3 scripts/gen_ws_docs.py

Requires: pip install jinja2 pyyaml
"""

import json
import re
import subprocess
import sys
from pathlib import Path

try:
    import jinja2
except ImportError:
    sys.exit("jinja2 is required: pip install jinja2")

try:
    import yaml
except ImportError:
    sys.exit("pyyaml is required: pip install pyyaml")

SCRIPT_DIR = Path(__file__).parent
REPO_ROOT = SCRIPT_DIR.parent
JSON_PATH = REPO_ROOT / "target" / "doc" / "camillalib.json"
GROUPS_CONFIG = SCRIPT_DIR / "ws_groups.yaml"
TEMPLATE_FILE = "ws_template.md.j2"
OUTPUT_PATH = REPO_ROOT / "websocket.md"

# Types documented separately (error section, or the type itself is a command enum).
SKIP_INLINE_TYPES = frozenset({
    "WsResult",   # documented as the error-responses section
    "WsCommand",  # is the command enum itself
    "WsReply",    # push-event types; variant links appear in subscription docs
})


def run_rustdoc() -> None:
    print("Running rustdoc...", file=sys.stderr)
    result = subprocess.run(
        [
            "cargo", "+nightly", "rustdoc", "--lib", "--",
            "-Z", "unstable-options",
            "--output-format", "json",
            "--document-private-items",
        ],
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        sys.exit(f"rustdoc failed:\n{result.stderr}")


def clean_docs(text: str) -> str:
    """Strip rustdoc intra-doc link syntax, leaving plain Markdown."""
    if not text:
        return ""
    # [`SomeType`](path::to::it) → `SomeType`
    text = re.sub(r'\[(`[^`]+`)\]\([^)]*\)', r'\1', text)
    # [`SomeType`] → `SomeType`
    text = re.sub(r'\[(`[^`]+`)\]', r'\1', text)
    return text


# Map Rust primitive/stdlib names to JSON type names.
_JSON_TYPES: dict[str, str] = {
    "bool": "boolean",
    "f32": "number",
    "f64": "number",
    "i8": "integer",
    "i16": "integer",
    "i32": "integer",
    "i64": "integer",
    "i128": "integer",
    "isize": "integer",
    "u8": "integer (≥ 0)",
    "u16": "integer (≥ 0)",
    "u32": "integer (≥ 0)",
    "u64": "integer (≥ 0)",
    "u128": "integer (≥ 0)",
    "usize": "integer (≥ 0)",
    "String": "string",
    "str": "string",
    "Value": "any",
}


def format_type_node(type_node: dict) -> str:
    """Convert a rustdoc JSON type node to a human-readable type string.

    Handles primitives, resolved paths (including generics), and tuples.
    Tuples are rendered with brackets since they serialise as JSON arrays.
    Rust-specific numeric/string types are mapped to their JSON equivalents.
    """
    if not isinstance(type_node, dict):
        return "?"
    if "primitive" in type_node:
        raw = type_node["primitive"]
        return _JSON_TYPES.get(raw, raw)
    if "resolved_path" in type_node:
        rp = type_node["resolved_path"]
        full_path = rp.get("path", rp.get("name", "?"))
        name = full_path.rsplit("::", 1)[-1]
        name = _JSON_TYPES.get(name, name)
        args = rp.get("args") or {}
        angle_args = args.get("angle_bracketed", {}).get("args", [])
        if angle_args:
            inner_types = [
                format_type_node(arg["type"])
                for arg in angle_args
                if "type" in arg
            ]
            if inner_types:
                if name == "Option":
                    return f"{inner_types[0]} | null"
                if name == "Vec":
                    return f"{inner_types[0]}[]"
                return f"{name}<{', '.join(inner_types)}>"
        return name
    if "tuple" in type_node:
        parts = [format_type_node(t) for t in type_node["tuple"] if t is not None]
        return f"[{', '.join(parts)}]"
    return "?"


def command_arg_type_str(index: dict, variant_item: dict) -> str:
    """Return a formatted argument type string for a WsCommand variant.

    For single-field tuple variants returns the type name; for multi-field
    variants returns a bracketed list (JSON array notation).
    Returns an empty string for unit variants.
    """
    kind = variant_item.get("inner", {}).get("variant", {}).get("kind", {})
    if not isinstance(kind, dict):
        return ""

    field_ids: list[int] = []
    if "tuple" in kind:
        field_ids = [fid for fid in kind["tuple"] if fid is not None]
    elif "struct" in kind:
        field_ids = kind["struct"].get("fields", [])

    if not field_ids:
        return ""

    type_strings = []
    for fid in field_ids:
        field = index.get(str(fid), {})
        sf = field.get("inner", {}).get("struct_field")
        if isinstance(sf, dict):
            type_strings.append(format_type_node(sf))

    if not type_strings:
        return ""
    if len(type_strings) == 1:
        return type_strings[0]
    return f"[{', '.join(type_strings)}]"


def inject_arg_type(docs: str, arg_type: str) -> str:
    """Inject a type annotation into 'Argument(s):' lines in a doc string.

    Turns 'Argument: description' into 'Argument: `type` — description'.
    Lines without an existing Argument(s) label are left unchanged.
    """
    if not arg_type or not docs:
        return docs

    def replace_match(m: re.Match) -> str:
        return f"{m.group(1)}: `{arg_type}` — {m.group(2).strip()}"

    return re.sub(r"(Arguments?):\s+(.+)", replace_match, docs)


def get_type_entries(index: dict, item_id: int) -> tuple[str, list[dict]] | None:
    """Return (kind, entries) for a struct (fields) or enum (variants), or None."""
    item = index.get(str(item_id))
    if not item:
        return None
    inner = item.get("inner", {})

    if "struct" in inner:
        kind = inner["struct"].get("kind", {})
        field_ids = kind.get("plain", {}).get("fields", [])
        entries = []
        for fid in field_ids:
            f = index.get(str(fid))
            if not f or not f.get("docs"):
                continue
            docs = clean_docs(f["docs"])
            sf = f.get("inner", {}).get("struct_field")
            type_str = format_type_node(sf) if isinstance(sf, dict) else ""
            entries.append({
                "name": f["name"],
                "docs": docs.split("\n\n")[0].strip(),
                "type": type_str,
            })
        return ("fields", entries) if entries else None

    if "enum" in inner:
        entries = []
        for vid in inner["enum"]["variants"]:
            v = index.get(str(vid))
            if not v or not v.get("docs"):
                continue
            docs = clean_docs(v["docs"])
            entries.append({"name": v["name"], "docs": docs.split("\n\n")[0].strip(), "type": ""})
        return ("values", entries) if entries else None

    return None


def linked_types(index: dict, variant_item: dict) -> list[dict]:
    """Collect documented structs/enums for a variant's argument types.

    Walks the actual Rust field types first (tuple or struct variant kinds),
    then also checks intra-doc links in the doc comment (for types only
    referenced in prose, not used directly as fields).
    """
    result = []
    seen: set[str] = set()

    def add_if_useful(item_id: int) -> None:
        item = index.get(str(item_id))
        if not item:
            return
        name = item.get("name", "")
        if name in SKIP_INLINE_TYPES or name in seen:
            return
        seen.add(name)
        info = get_type_entries(index, item_id)
        if info:
            kind, entries = info
            result.append({"name": name, "kind": kind, "entries": entries})

    # Walk actual Rust field types from the variant definition.
    kind = variant_item.get("inner", {}).get("variant", {}).get("kind", {})
    field_ids: list[int] = []
    if isinstance(kind, dict):
        if "tuple" in kind:
            field_ids = [fid for fid in kind["tuple"] if fid is not None]
        elif "struct" in kind:
            field_ids = kind["struct"].get("fields", [])
    for fid in field_ids:
        field = index.get(str(fid), {})
        sf = field.get("inner", {}).get("struct_field")
        if isinstance(sf, dict):
            rp = sf.get("resolved_path")
            if isinstance(rp, dict) and "id" in rp:
                add_if_useful(rp["id"])

    # Also check intra-doc links for types only mentioned in prose.
    for item_id in variant_item.get("links", {}).values():
        add_if_useful(item_id)

    return result


def _resolved_path_ids(sf: dict) -> list[int]:
    """Return item IDs to try for type expansion from a struct_field type node.

    Checks the top-level resolved_path, then the first generic type argument
    (handles Vec<T>, Option<T>, etc. where T is the interesting type).
    """
    ids: list[int] = []
    rp = sf.get("resolved_path")
    if isinstance(rp, dict) and "id" in rp:
        ids.append(rp["id"])
        # Also look one level into generic args (e.g. Vec<Fader> → Fader).
        args = rp.get("args") or {}
        for arg in args.get("angle_bracketed", {}).get("args", []):
            inner_rp = arg.get("type", {}).get("resolved_path")
            if isinstance(inner_rp, dict) and "id" in inner_rp:
                ids.append(inner_rp["id"])
    return ids


def reply_value_info(index: dict, reply_variant: dict) -> dict:
    """Extract the value field doc and any expandable return type from a WsReply variant."""
    kind = reply_variant.get("inner", {}).get("variant", {}).get("kind", {})
    if not isinstance(kind, dict) or "struct" not in kind:
        return {}
    for fid in kind["struct"].get("fields", []):
        field = index.get(str(fid), {})
        if field.get("name") != "value":
            continue
        doc = clean_docs(field.get("docs") or "")
        types: list[dict] = []
        returns_type_str = ""
        sf = field.get("inner", {}).get("struct_field")
        if isinstance(sf, dict):
            returns_type_str = format_type_node(sf)
            for item_id in _resolved_path_ids(sf):
                type_item = index.get(str(item_id))
                if not type_item:
                    continue
                tname = type_item.get("name", "")
                if tname in SKIP_INLINE_TYPES:
                    continue
                info = get_type_entries(index, item_id)
                if info:
                    tkind, entries = info
                    types.append({"name": tname, "kind": tkind, "entries": entries})
                    break  # expand only the first documentable type
        return {"returns_doc": doc, "returns_types": types, "returns_type_str": returns_type_str}
    return {}


def extract_reply_values(index: dict) -> dict[str, dict]:
    """Map each WsReply variant name to its return value info."""
    for item in index.values():
        if item.get("name") == "WsReply" and "enum" in item.get("inner", {}):
            result = {}
            for vid in item["inner"]["enum"]["variants"]:
                v = index[str(vid)]
                info = reply_value_info(index, v)
                if info:
                    result[v["name"]] = info
            return result
    return {}


def extract_enum_variants(index: dict, enum_name: str) -> list[dict]:
    for item in index.values():
        if item.get("name") == enum_name and "enum" in item.get("inner", {}):
            variants = []
            for vid in item["inner"]["enum"]["variants"]:
                v = index[str(vid)]
                variants.append({
                    "name": v["name"],
                    "docs": clean_docs(v.get("docs") or ""),
                    "linked_types": linked_types(index, v),
                    "_item": v,
                })
            return variants
    sys.exit(f"Could not find enum '{enum_name}' in rustdoc JSON")


def main() -> None:
    run_rustdoc()

    data = json.loads(JSON_PATH.read_text())
    config = yaml.safe_load(GROUPS_CONFIG.read_text())
    index = data["index"]

    # --- WsCommand variants + return info from matching WsReply variants ---
    all_variants = extract_enum_variants(index, "WsCommand")
    reply_values = extract_reply_values(index)
    for v in all_variants:
        info = reply_values.get(v["name"], {})
        v["returns_doc"] = info.get("returns_doc", "")
        v["returns_types"] = info.get("returns_types", [])
        # Build returns_line: "`type` — doc" or just one of them if the other is absent.
        rtype = info.get("returns_type_str", "")
        rdoc = v["returns_doc"]
        if rtype and rdoc:
            v["returns_line"] = f"`{rtype}` — {rdoc}"
        elif rtype:
            v["returns_line"] = f"`{rtype}`"
        else:
            v["returns_line"] = rdoc
        # Inject argument type into "Argument(s):" lines in the command docs.
        arg_type = command_arg_type_str(index, v.pop("_item", {}))
        if arg_type:
            v["docs"] = inject_arg_type(v["docs"], arg_type)
    variant_map = {v["name"]: v for v in all_variants}

    # Build groups, collecting all command names mentioned in config
    config_names: set[str] = set()
    groups = []
    ok = True
    for group in config["groups"]:
        commands = []
        for name in group["commands"]:
            config_names.add(name)
            if name not in variant_map:
                print(
                    f"Warning: ws_groups.yaml references '{name}' "
                    "which does not exist in WsCommand",
                    file=sys.stderr,
                )
                ok = False
            else:
                commands.append(variant_map[name])
        groups.append({
            "name": group["name"],
            "description": group.get("description", "").strip(),
            "commands": commands,
        })

    # Warn about commands present in Rust but absent from config
    for name in variant_map:
        if name not in config_names:
            print(
                f"Warning: WsCommand::{name} is not listed in ws_groups.yaml",
                file=sys.stderr,
            )
            ok = False

    if not ok:
        print(
            "Update scripts/ws_groups.yaml to resolve the warnings above.",
            file=sys.stderr,
        )

    # --- WsResult error variants (skip Ok) ---
    all_errors = extract_enum_variants(index, "WsResult")
    errors = [e for e in all_errors if e["name"] != "Ok"]

    # --- Render ---
    env = jinja2.Environment(
        loader=jinja2.FileSystemLoader(str(SCRIPT_DIR)),
        keep_trailing_newline=True,
        trim_blocks=True,
        lstrip_blocks=True,
    )
    template = env.get_template(TEMPLATE_FILE)
    OUTPUT_PATH.write_text(template.render(groups=groups, errors=errors))
    print(f"Written to {OUTPUT_PATH}", file=sys.stderr)


if __name__ == "__main__":
    main()
