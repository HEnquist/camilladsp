#!/usr/bin/env python3
"""
Generate websocket API documentation from the OpenAPI schemas of the protocol types.

The schemas come from the ws_schema example of the camilladsp-schema crate, which utoipa
derives from the Rust types and their serde attributes, so all names are the wire names.

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
GROUPS_CONFIG = SCRIPT_DIR / "ws_groups.yaml"
TEMPLATE_FILE = "ws_template.md.j2"
OUTPUT_PATH = REPO_ROOT / "websocket.md"

# The message enums themselves, documented as commands, replies and errors rather than as types.
MESSAGE_TYPES = frozenset({"WsCommand", "WsReply", "WsResult"})

# Commands that exist in WsCommand but are not part of the protocol.
INTERNAL_COMMANDS = frozenset({
    "None",  # sentinel for non-text frames, #[doc(hidden)]
})


def load_schemas() -> dict:
    print("Generating the protocol schemas...", file=sys.stderr)
    result = subprocess.run(
        [
            "cargo", "run", "-q", "-p", "camilladsp-schema",
            "--features", "utoipa", "--example", "ws_schema",
        ],
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        sys.exit(f"ws_schema failed:\n{result.stderr}")
    return json.loads(result.stdout)["components"]["schemas"]


def clean_docs(text: str) -> str:
    """Strip rustdoc intra-doc link syntax, leaving plain Markdown."""
    if not text:
        return ""
    # [`SomeType`](path::to::it) → `SomeType`
    text = re.sub(r'\[(`[^`]+`)\]\([^)]*\)', r'\1', text)
    # [`SomeType`] → `SomeType`
    text = re.sub(r'\[(`[^`]+`)\]', r'\1', text)
    return text.strip()


def one_line(text: str) -> str:
    """Join a multi-paragraph doc into one line, for use in a list entry."""
    return re.sub(r"\s*\n\s*", " ", clean_docs(text))


def anchor(name: str) -> str:
    """The GitHub heading anchor for a heading that is just the name in backticks."""
    return name.lower()


def ref_name(schema: dict) -> str | None:
    ref = schema.get("$ref")
    return ref.rsplit("/", 1)[-1] if ref else None


def nullable_inner(schema: dict) -> dict | None:
    """For a `oneOf: [X, {type: null}]` schema, return X."""
    options = schema.get("oneOf")
    if options and len(options) == 2 and {"type": "null"} in options:
        return next(o for o in options if o != {"type": "null"})
    return None


def description_of(schema: dict) -> str:
    """The description of a schema, also looking inside a nullable wrapper."""
    if "description" in schema:
        return schema["description"]
    inner = nullable_inner(schema)
    return inner.get("description", "") if inner else ""


def primitive_md(type_name: str, schema: dict) -> str:
    if type_name == "integer" and "minimum" in schema:
        return f"`integer (≥ {schema['minimum']})`"
    return f"`{type_name}`"


def type_md(schema: dict) -> str:
    """Render a schema as a Markdown type expression, linking named types to the Types section."""
    name = ref_name(schema)
    if name:
        return f"[`{name}`](#{anchor(name)})"
    inner = nullable_inner(schema)
    if inner is not None:
        return f"{type_md(inner)} or `null`"
    type_field = schema.get("type")
    if isinstance(type_field, list):
        types = [t for t in type_field if t != "null"]
        rendered = " or ".join(type_md({**schema, "type": t}) for t in types)
        return f"{rendered} or `null`" if "null" in type_field else rendered
    if type_field == "array":
        if "prefixItems" in schema:
            parts = ", ".join(type_md(item) for item in schema["prefixItems"])
            return f"array \\[{parts}\\]"
        items = type_md(schema.get("items", {}))
        return f"array of ({items})" if " or " in items else f"array of {items}"
    if type_field:
        return primitive_md(type_field, schema)
    if not set(schema) - {"description"}:
        return "any JSON value"
    print(f"Warning: unhandled schema shape {json.dumps(schema)}", file=sys.stderr)
    return "`?`"


def referenced_types(schema, found: list[str]) -> None:
    """Collect the names of all named types a schema refers to, in order of appearance."""
    if isinstance(schema, dict):
        name = ref_name(schema)
        if name and name not in found:
            found.append(name)
        for value in schema.values():
            referenced_types(value, found)
    elif isinstance(schema, list):
        for value in schema:
            referenced_types(value, found)


def field_entries(schema: dict, skip: frozenset[str] = frozenset()) -> list[dict]:
    """List the properties of an object schema, in declaration order."""
    required = set(schema.get("required", []))
    return [
        {
            "name": name,
            "type": type_md(prop),
            "optional": name not in required,
            "docs": one_line(description_of(prop)),
        }
        for name, prop in schema.get("properties", {}).items()
        if name not in skip
    ]


def tag_value(schema: dict, tag: str) -> str | None:
    """The tag value of one variant of an internally tagged enum."""
    parts = schema.get("allOf", [schema])
    for part in parts:
        prop = part.get("properties", {}).get(tag)
        if prop and "enum" in prop:
            return prop["enum"][0]
    return None


def variant_properties(schema: dict, tag: str) -> dict:
    """Merge the properties of a variant of an internally tagged enum, leaving out the tag
    and the flattened WsResult."""
    merged = {"properties": {}, "required": []}
    for part in schema.get("allOf", [schema]):
        if ref_name(part):
            continue
        for name, prop in part.get("properties", {}).items():
            if name != tag:
                merged["properties"][name] = prop
        merged["required"] += [r for r in part.get("required", []) if r != tag]
    return merged


def describe_type(name: str, schema: dict) -> dict:
    """Describe one named type for the Types section."""
    # "options" rather than "values", which in the template would be the dict method.
    entry = {"name": name, "docs": clean_docs(schema.get("description", "")), "options": [],
             "fields": [], "plain_values": ""}
    if "enum" in schema:
        # A plain enum has no per-value docs, so its values go on one line.
        entry["plain_values"] = ", ".join(f"`{json.dumps(v)}`" for v in schema["enum"])
    elif "oneOf" in schema:
        for variant in schema["oneOf"]:
            docs = one_line(variant.get("description", ""))
            if "enum" in variant:
                entry["options"].append({"name": json.dumps(variant["enum"][0]), "docs": docs})
            else:
                # An externally tagged variant with data, an object with a single key.
                [(key, prop)] = variant["properties"].items()
                plain_type = re.sub(r"[`\\]|\]\(#[^)]*\)|\[", "", type_md(prop))
                entry["options"].append({"name": f'{{"{key}": {plain_type}}}', "docs": docs})
    else:
        entry["fields"] = field_entries(schema)
    return entry


def command_entry(schema: dict, replies: dict[str, dict]) -> dict:
    name = tag_value(schema, "command")
    docs = clean_docs(schema.get("description", ""))
    args = field_entries(schema, skip=frozenset({"command"}))

    returns = ""
    reply = replies.get(name)
    if reply is not None:
        value = variant_properties(reply, "reply")["properties"].get("value")
        if value is not None:
            value_docs = one_line(description_of(value))
            returns = f"({type_md(value)}): {value_docs}" if value_docs else type_md(value)
    return {"name": name, "docs": docs, "args": args, "returns": returns}


def other_reply_entry(name: str, schema: dict) -> dict:
    return {
        "name": name,
        "docs": clean_docs(schema.get("description", "")),
        "fields": field_entries(variant_properties(schema, "reply")),
    }


def main() -> None:
    schemas = load_schemas()
    config = yaml.safe_load(GROUPS_CONFIG.read_text())

    replies = {tag_value(r, "reply"): r for r in schemas["WsReply"]["oneOf"]}
    commands = [
        command_entry(c, replies)
        for c in schemas["WsCommand"]["oneOf"]
        if tag_value(c, "command") not in INTERNAL_COMMANDS
    ]
    command_map = {c["name"]: c for c in commands}

    # Build groups, collecting all command names mentioned in config
    config_names: set[str] = set()
    groups = []
    ok = True
    for group in config["groups"]:
        group_commands = []
        for name in group["commands"]:
            config_names.add(name)
            if name not in command_map:
                print(
                    f"Warning: ws_groups.yaml references '{name}' "
                    "which does not exist in WsCommand",
                    file=sys.stderr,
                )
                ok = False
            else:
                group_commands.append(command_map[name])
        groups.append({
            "name": group["name"],
            "description": group.get("description", "").strip(),
            "commands": group_commands,
        })

    # Warn about commands present in Rust but absent from config
    for name in command_map:
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

    # Replies that do not answer a command: pushed events, and the reply to an invalid message.
    other_replies = [
        other_reply_entry(name, schema)
        for name, schema in replies.items()
        if name not in command_map and name not in INTERNAL_COMMANDS
    ]

    # Errors are the WsResult variants other than Ok.
    errors = [
        {"name": tag_value(e, "result"), "docs": clean_docs(e.get("description", ""))}
        for e in schemas["WsResult"]["oneOf"]
        if tag_value(e, "result") != "Ok"
    ]

    # Every named type the messages use, in order of first appearance in the command groups.
    command_schemas = {tag_value(c, "command"): c for c in schemas["WsCommand"]["oneOf"]}
    type_names: list[str] = []
    for group in groups:
        for command in group["commands"]:
            referenced_types(command_schemas[command["name"]], type_names)
            if command["name"] in replies:
                referenced_types(replies[command["name"]], type_names)
    for reply in other_replies:
        referenced_types(replies[reply["name"]], type_names)
    # Follow the references inside the types themselves.
    i = 0
    while i < len(type_names):
        referenced_types(schemas[type_names[i]], type_names)
        i += 1
    types = [describe_type(n, schemas[n]) for n in type_names if n not in MESSAGE_TYPES]

    # The type headings and the command headings share one anchor namespace.
    clashes = {t["name"].lower() for t in types} & {c.lower() for c in command_map}
    if clashes:
        sys.exit(f"Type names clash with command names: {', '.join(sorted(clashes))}")

    # --- Render ---
    env = jinja2.Environment(
        loader=jinja2.FileSystemLoader(str(SCRIPT_DIR)),
        keep_trailing_newline=True,
        trim_blocks=True,
        lstrip_blocks=True,
    )
    template = env.get_template(TEMPLATE_FILE)
    OUTPUT_PATH.write_text(template.render(
        groups=groups, other_replies=other_replies, types=types, errors=errors,
    ))
    print(f"Written to {OUTPUT_PATH}", file=sys.stderr)


if __name__ == "__main__":
    main()
