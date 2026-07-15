import os
from pathlib import Path
import hashlib
import re
import shutil
import subprocess
import sys
import tempfile

errors = []


def workspace_root():
    candidate = os.environ.get("CANARY_CI_SOURCE_ROOT") or os.environ.get(
        "GITHUB_WORKSPACE"
    )

    if candidate:
        return Path(candidate)

    return Path.cwd()


def read_required_text(path, label):
    try:
        return path.read_text()
    except OSError as exc:
        errors.append(f"unable to read {label} at {path}: {exc}")
        return ""


root = workspace_root()
policy_root = Path(os.environ.get("CANARY_CI_POLICY_ROOT", root))
workflow = read_required_text(root / ".github/workflows/ci.yml", "GitHub workflow")
uptime_workflow = read_required_text(
    root / ".github/workflows/uptime-monitor.yml", "witness workflow"
)
dagger_source = read_required_text(root / "dagger/src/index.ts", "Dagger source")
dagger_config = read_required_text(root / "dagger.json", "Dagger config")
dagger_wrapper = read_required_text(root / "bin/dagger", "Dagger wrapper")
source_argument_sync = policy_root / "dagger/scripts/sync_source_arguments.py"
real_path = os.environ.get("PATH", "")
real_bash = shutil.which("bash", path=real_path) or "/bin/bash"
real_uname = shutil.which("uname", path=real_path) or "/usr/bin/uname"


def require(condition, message):
    if not condition:
        errors.append(message)


def path_with_shims(*shim_dirs):
    entries = [str(path) for path in shim_dirs if path]
    entries.extend(entry for entry in real_path.split(os.pathsep) if entry)
    return os.pathsep.join(dict.fromkeys(entries))


def shadow_missing_command(shim_dir, command_name):
    shadow_path = shim_dir / command_name
    shadow_path.write_text("#!/usr/bin/env bash\nexit 127\n")
    shadow_path.chmod(0o755)


def reset_shadow_commands(shim_dir):
    for shadow_path in shim_dir.iterdir():
        shadow_path.unlink()


class ContractParseError(ValueError):
    pass


CLASS_METHOD_MODIFIERS = {
    "private",
    "protected",
    "public",
    "readonly",
    "static",
    "async",
}


def is_identifier_start(char):
    return char.isalpha() or char in {"_", "$"}


def is_identifier_part(char):
    return char.isalnum() or char in {"_", "$"}


def skip_line_comment(source_text, index):
    while index < len(source_text) and source_text[index] != "\n":
        index += 1
    return index


def skip_block_comment(source_text, index):
    end = source_text.find("*/", index + 2)

    if end == -1:
        raise ContractParseError("unterminated block comment")

    return end + 2


def skip_quoted_string(source_text, index, quote):
    index += 1

    while index < len(source_text):
        char = source_text[index]

        if char == "\\":
            index += 2
            continue

        if char == quote:
            return index + 1

        index += 1

    raise ContractParseError(f"unterminated string literal starting with {quote}")


def skip_template_expression(source_text, index):
    depth = 1

    while index < len(source_text):
        new_index = skip_non_code(source_text, index)

        if new_index != index:
            index = new_index
            continue

        char = source_text[index]

        if char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                return index + 1

        index += 1

    raise ContractParseError("unterminated template expression")


def skip_template_literal(source_text, index):
    index += 1

    while index < len(source_text):
        char = source_text[index]

        if char == "\\":
            index += 2
            continue

        if char == "`":
            return index + 1

        if char == "$" and index + 1 < len(source_text) and source_text[index + 1] == "{":
            index = skip_template_expression(source_text, index + 2)
            continue

        index += 1

    raise ContractParseError("unterminated template literal")


def skip_non_code(source_text, index):
    if source_text.startswith("//", index):
        return skip_line_comment(source_text, index)

    if source_text.startswith("/*", index):
        return skip_block_comment(source_text, index)

    char = source_text[index]

    if char in {"'", '"'}:
        return skip_quoted_string(source_text, index, char)

    if char == "`":
        return skip_template_literal(source_text, index)

    return index


def skip_space_and_comments(source_text, index):
    while index < len(source_text):
        char = source_text[index]

        if char.isspace():
            index += 1
            continue

        new_index = skip_non_code(source_text, index)

        if new_index != index and source_text[index] in {"/", "'", '"', "`"}:
            index = new_index
            continue

        return index

    return index


def read_identifier(source_text, index):
    if index >= len(source_text) or not is_identifier_start(source_text[index]):
        return None, index

    end = index + 1

    while end < len(source_text) and is_identifier_part(source_text[end]):
        end += 1

    return source_text[index:end], end


def find_matching(source_text, index, open_char, close_char):
    if index >= len(source_text) or source_text[index] != open_char:
        raise ContractParseError(f"expected {open_char} at offset {index}")

    depth = 1
    index += 1

    while index < len(source_text):
        new_index = skip_non_code(source_text, index)

        if new_index != index:
            index = new_index
            continue

        char = source_text[index]

        if char == open_char:
            depth += 1
        elif char == close_char:
            depth -= 1
            if depth == 0:
                return index

        index += 1

    raise ContractParseError(f"unterminated {open_char}{close_char} block")


def find_class_body(source_text, class_name):
    class_match = re.search(rf"\bclass\s+{re.escape(class_name)}\b", source_text)

    if class_match is None:
        raise ContractParseError(f"missing class {class_name}")

    brace_start = source_text.find("{", class_match.end())

    if brace_start == -1:
        raise ContractParseError(f"missing body for class {class_name}")

    brace_end = find_matching(source_text, brace_start, "{", "}")
    return source_text[brace_start + 1 : brace_end]


def parse_decorator(source_text, index):
    index += 1
    index = skip_space_and_comments(source_text, index)
    name, index = read_identifier(source_text, index)

    if name is None:
        raise ContractParseError("decorator is missing its name")

    index = skip_space_and_comments(source_text, index)

    if index < len(source_text) and source_text[index] == "(":
        index = find_matching(source_text, index, "(", ")") + 1

    return name, index


def find_method_body_start(source_text, index):
    index = skip_space_and_comments(source_text, index)

    if index >= len(source_text):
        raise ContractParseError("method signature is missing a body")

    if source_text[index] == "{":
        return index

    if source_text[index] != ":":
        raise ContractParseError("method signature is missing a return type/body separator")

    index += 1
    paren_depth = 0
    bracket_depth = 0
    angle_depth = 0

    while index < len(source_text):
        new_index = skip_non_code(source_text, index)

        if new_index != index:
            index = new_index
            continue

        char = source_text[index]

        if char == "(":
            paren_depth += 1
        elif char == ")":
            paren_depth = max(paren_depth - 1, 0)
        elif char == "[":
            bracket_depth += 1
        elif char == "]":
            bracket_depth = max(bracket_depth - 1, 0)
        elif char == "<":
            angle_depth += 1
        elif char == ">":
            angle_depth = max(angle_depth - 1, 0)
        elif (
            char == "{"
            and paren_depth == 0
            and bracket_depth == 0
            and angle_depth == 0
        ):
            return index

        index += 1

    raise ContractParseError("method signature is missing its body")


def parse_class_method(source_text, index, decorators):
    cursor = index
    modifiers = []

    while True:
        cursor = skip_space_and_comments(source_text, cursor)
        modifier, next_cursor = read_identifier(source_text, cursor)

        if modifier in CLASS_METHOD_MODIFIERS:
            modifiers.append(modifier)
            cursor = next_cursor
            continue

        break

    name, cursor = read_identifier(source_text, cursor)

    if name is None:
        return None, index + 1

    cursor = skip_space_and_comments(source_text, cursor)

    if cursor >= len(source_text) or source_text[cursor] != "(":
        return None, index + 1

    params_end = find_matching(source_text, cursor, "(", ")")
    body_start = find_method_body_start(source_text, params_end + 1)
    body_end = find_matching(source_text, body_start, "{", "}")

    return (
        {
            "name": name,
            "decorators": tuple(decorators),
            "modifiers": tuple(modifiers),
            "body": source_text[body_start + 1 : body_end],
        },
        body_end + 1,
    )


def parse_ci_methods(source_text):
    class_body = find_class_body(source_text, "Ci")
    methods = []
    index = 0

    while index < len(class_body):
        index = skip_space_and_comments(class_body, index)

        if index >= len(class_body):
            break

        decorators = []

        while index < len(class_body) and class_body[index] == "@":
            decorator_name, index = parse_decorator(class_body, index)
            decorators.append(decorator_name)
            index = skip_space_and_comments(class_body, index)

        method, next_index = parse_class_method(class_body, index, decorators)

        if method is None:
            index += 1
            continue

        methods.append(method)
        index = next_index

    return methods


def extract_named_body(source_text, function_name):
    methods = [
        item for item in parse_ci_methods(source_text) if item["name"] == function_name
    ]
    if len(methods) > 1:
        raise ContractParseError(f"duplicate Ci method body for {function_name}")
    if methods:
        return methods[0]["body"]

    matches = list(
        re.finditer(
        rf"\b(?:async\s+)?function\s+{re.escape(function_name)}\s*\(",
        source_text,
        )
    )
    if len(matches) > 1:
        raise ContractParseError(f"duplicate top-level function body for {function_name}")
    if not matches:
        raise ContractParseError(f"missing function body for {function_name}")
    match = matches[0]
    arguments_start = match.end() - 1
    arguments_end = find_matching(source_text, arguments_start, "(", ")")
    index = arguments_end + 1
    while index < len(source_text):
        next_index = skip_non_code(source_text, index)
        if next_index != index:
            index = next_index
            continue
        if source_text[index] == "{":
            body_end = find_matching(source_text, index, "{", "}")
            return source_text[index + 1 : body_end]
        index += 1
    raise ContractParseError(f"missing opening body brace for {function_name}")


def compact_code(source_text):
    compact = []
    index = 0
    while index < len(source_text):
        if source_text.startswith("//", index):
            newline = source_text.find("\n", index + 2)
            index = len(source_text) if newline == -1 else newline + 1
            continue
        if source_text.startswith("/*", index):
            end = source_text.find("*/", index + 2)
            if end == -1:
                raise ContractParseError("unterminated block comment")
            index = end + 2
            continue
        character = source_text[index]
        if character in {'"', "'", "`"}:
            quote = character
            start = index
            index += 1
            while index < len(source_text):
                if source_text[index] == "\\":
                    index += 2
                    continue
                if source_text[index] == quote:
                    index += 1
                    compact.append(source_text[start:index])
                    break
                index += 1
            else:
                raise ContractParseError("unterminated string literal")
            continue
        if character.isspace():
            index += 1
            continue
        compact.append(character)
        index += 1
    return "".join(compact)


def body_digest(source_text, function_name):
    body = extract_named_body(source_text, function_name)
    return hashlib.sha256(compact_code(body).encode()).hexdigest()


def top_level_const_expression(source_text, constant_name):
    match = re.search(
        rf"^const\s+{re.escape(constant_name)}\s*=\s*(.*?)\s*$",
        source_text,
        re.MULTILINE,
    )
    if match is None:
        return None
    return re.sub(r"\s+", "", match.group(1))


def extract_top_level_const_bindings(method_body):
    bindings = []
    for line in method_body.splitlines():
        match = re.match(r"^\s*const\s+(\w+)\s*=\s*(.*?)\s*;?\s*$", line)
        if match is None:
            continue
        bindings.append(
            {
                "name": match.group(1),
                "expression": re.sub(r"\s+", "", match.group(2)),
            }
        )
    return bindings


def compact_expression(source_text):
    compact = []
    index = 0

    while index < len(source_text):
        new_index = skip_non_code(source_text, index)

        if new_index != index:
            index = new_index
            continue

        char = source_text[index]

        if not char.isspace():
            compact.append(char)

        index += 1

    return "".join(compact)


def parse_await_this_call(source_text, index=0):
    index = skip_space_and_comments(source_text, index)
    first_ident, index = read_identifier(source_text, index)

    if first_ident != "await":
        return None, index

    index = skip_space_and_comments(source_text, index)
    receiver, index = read_identifier(source_text, index)

    if receiver != "this":
        return None, index

    index = skip_space_and_comments(source_text, index)

    if index >= len(source_text) or source_text[index] != ".":
        return None, index

    index += 1
    index = skip_space_and_comments(source_text, index)
    method_name, index = read_identifier(source_text, index)

    if method_name is None:
        raise ContractParseError("awaited this-call is missing its method name")

    index = skip_space_and_comments(source_text, index)

    if index >= len(source_text) or source_text[index] != "(":
        raise ContractParseError(f"await this.{method_name} call is missing its argument list")

    args_end = find_matching(source_text, index, "(", ")")
    arguments = compact_expression(source_text[index + 1 : args_end]).rstrip(",")
    return {"name": method_name, "arguments": arguments}, args_end + 1


def extract_strict_invocations(strict_body):
    invocations = []
    index = 0
    paren_depth = 0
    bracket_depth = 0
    brace_depth = 0

    while index < len(strict_body):
        new_index = skip_non_code(strict_body, index)

        if new_index != index:
            index = new_index
            continue

        char = strict_body[index]

        if char == "(":
            paren_depth += 1
            index += 1
            continue

        if char == ")":
            paren_depth = max(paren_depth - 1, 0)
            index += 1
            continue

        if char == "[":
            bracket_depth += 1
            index += 1
            continue

        if char == "]":
            bracket_depth = max(bracket_depth - 1, 0)
            index += 1
            continue

        if char == "{":
            brace_depth += 1
            index += 1
            continue

        if char == "}":
            brace_depth = max(brace_depth - 1, 0)
            index += 1
            continue

        if (
            paren_depth == 0
            and bracket_depth == 0
            and brace_depth == 0
            and is_identifier_start(char)
        ):
            identifier, next_index = read_identifier(strict_body, index)

            if identifier == "await":
                invocation, call_end = parse_await_this_call(strict_body, index)

                if invocation is not None:
                    invocations.append(invocation)
                    index = call_end
                    continue

            index = next_index
            continue

        index += 1

    return invocations


def extract_ci_contract(source_text):
    methods = parse_ci_methods(source_text)
    method_names = [method["name"] for method in methods]
    duplicate_method_names = sorted(
        {name for name in method_names if method_names.count(name) > 1}
    )
    if duplicate_method_names:
        raise ContractParseError(
            f"duplicate Ci method names: {', '.join(duplicate_method_names)}"
        )
    check_methods = [method["name"] for method in methods if "check" in method["decorators"]]
    strict_method = next((method for method in methods if method["name"] == "strict"), None)

    if strict_method is None:
        raise ContractParseError("missing Ci.strict method")

    strict_invocations = extract_strict_invocations(strict_method["body"])
    return {
        "check_methods": check_methods,
        "strict_calls": [invocation["name"] for invocation in strict_invocations],
        "strict_invocations": strict_invocations,
        "strict_bindings": extract_top_level_const_bindings(strict_method["body"]),
    }


def format_call_sequence(calls):
    return " -> ".join(calls) if calls else "(none)"


def strict_contract_message(expected_calls, actual_calls):
    message_parts = [
        "Ci.strict must resolve scope, execute codexAgentRoles and every @check gate in source order, then advisories",
        f"expected: {format_call_sequence(expected_calls)}",
        f"actual: {format_call_sequence(actual_calls)}",
    ]
    missing = [call for call in expected_calls if call not in actual_calls]
    extra = [call for call in actual_calls if call not in expected_calls]

    if missing:
        message_parts.append(f"missing: {', '.join(missing)}")

    if extra:
        message_parts.append(f"extra: {', '.join(extra)}")

    if not missing and not extra and expected_calls != actual_calls:
        out_of_order = []

        for index, expected in enumerate(expected_calls):
            actual = actual_calls[index] if index < len(actual_calls) else "(missing)"

            if actual != expected:
                out_of_order.append(f"step {index + 1}: expected {expected}, got {actual}")

        if out_of_order:
            message_parts.append("order: " + "; ".join(out_of_order))

    return "; ".join(message_parts)


def require_parser_fixture(
    label,
    source_text,
    expected_checks,
    expected_calls,
    expected_arguments=None,
    expected_bindings=None,
):
    try:
        contract = extract_ci_contract(source_text)
    except ContractParseError as exc:
        errors.append(f"{label}: parser raised {exc}")
        return

    require(
        contract["check_methods"] == expected_checks,
        f"{label}: expected @check gates {expected_checks}, got {contract['check_methods']}",
    )
    require(
        contract["strict_calls"] == expected_calls,
        f"{label}: expected strict calls {expected_calls}, got {contract['strict_calls']}",
    )
    require(
        [invocation["arguments"] for invocation in contract["strict_invocations"]]
        == (expected_arguments or ["repo"] * len(expected_calls)),
        f"{label}: strict call arguments drifted: {contract['strict_invocations']}",
    )
    if expected_bindings is not None:
        require(
            contract["strict_bindings"] == expected_bindings,
            f"{label}: strict authority bindings drifted: {contract['strict_bindings']}",
        )


fixture_reformatted_ci = """
@object()
export class Ci {
  @func()
  async strict(
    source?: Directory,
  ): Promise<void> {
    const repo = source!

    await this.codexAgentRoles(
      repo,
    )
    await this.deterministic(
      repo,
    )
    await this.secretsHistory(
      repo,
    )
    await this.advisories(repo)
  }

  @func()
  @check()
  async deterministic(
    source?: Directory,
  ): Promise<void> {
    await this.rustQuality(source!)
  }

  @func()
  @check()
  public async secretsHistory(
    source?: Directory,
  ): Promise<void> {
    await this.secrets(source!)
  }
}
"""

fixture_added_gate = """
@object()
export class Ci {
  @func()
  async strict(source?: Directory): Promise<void> {
    const repo = source!
    await this.codexAgentRoles(repo)
    await this.deterministic(repo)
    await this.openapiContract(repo)
    await this.secretsHistory(repo)
    await this.advisories(repo)
  }

  @func()
  @check()
  async deterministic(source?: Directory): Promise<void> {
    await this.rustQuality(source!)
  }

  @func()
  @check()
  async openapiContract(source?: Directory): Promise<void> {
    await this.ciContract(source!)
  }

  @func()
  @check()
  async secretsHistory(source?: Directory): Promise<void> {
    await this.secrets(source!)
  }
}
"""

fixture_scoped_ci = """
@object()
export class Ci {
  @func()
  async strict(source?: Directory, base?: Directory): Promise<void> {
    const repo = source!
    const candidate = repo.withoutDirectory(".git")
    const policy = (base ?? repo).withoutDirectory(".git")
    const scope = await this.resolveChangeScope(repo, base)
    await this.codexAgentRoles(candidate)
    await this.deterministic(candidate, policy, scope.runtime_required)
    await this.secretsHistory(repo)
    await this.advisoriesForScope(candidate, scope.runtime_required)
  }

  @func()
  @check()
  async deterministic(source?: Directory): Promise<void> {}

  @func()
  @check()
  async secretsHistory(source?: Directory): Promise<void> {}
}
"""

require_parser_fixture(
    "parser handles reformatted strict and decorator spacing",
    fixture_reformatted_ci,
    ["deterministic", "secretsHistory"],
    ["codexAgentRoles", "deterministic", "secretsHistory", "advisories"],
)
require_parser_fixture(
    "parser discovers newly-added @check gates from class structure",
    fixture_added_gate,
    ["deterministic", "openapiContract", "secretsHistory"],
    [
        "codexAgentRoles",
        "deterministic",
        "openapiContract",
        "secretsHistory",
        "advisories",
    ],
)
require_parser_fixture(
    "parser pins scoped strict arguments",
    fixture_scoped_ci,
    ["deterministic", "secretsHistory"],
    [
        "resolveChangeScope",
        "codexAgentRoles",
        "deterministic",
        "secretsHistory",
        "advisoriesForScope",
    ],
    [
        "repo,base",
        "candidate",
        "candidate,policy,scope.runtime_required",
        "repo",
        "candidate,scope.runtime_required",
    ],
    [
        {"name": "repo", "expression": "source!"},
        {"name": "candidate", "expression": 'repo.withoutDirectory(".git")'},
        {
            "name": "policy",
            "expression": '(base??repo).withoutDirectory(".git")',
        },
        {
            "name": "scope",
            "expression": "awaitthis.resolveChangeScope(repo,base)",
        },
    ],
)
scoped_policy_rebinding = extract_ci_contract(
    fixture_scoped_ci.replace(
        'const policy = (base ?? repo).withoutDirectory(".git")',
        "const policy = candidate",
    )
)
require(
    scoped_policy_rebinding["strict_bindings"]
    != extract_ci_contract(fixture_scoped_ci)["strict_bindings"],
    "strict binding parser must detect candidate-as-policy rebinding",
)
require(
    hashlib.sha256(compact_code("return trusted()\n").encode()).hexdigest()
    != hashlib.sha256(
        compact_code("return candidate()\n// return trusted()\n").encode()
    ).hexdigest(),
    "control-plane body digests must ignore comment decoys while detecting reachable changes",
)
try:
    extract_ci_contract(
        fixture_scoped_ci.replace(
            "\n}\n",
            "\n  private async resolveChangeScope(): Promise<void> {}\n"
            "  private async resolveChangeScope(): Promise<void> {}\n}\n",
        )
    )
except ContractParseError as exc:
    require(
        "duplicate Ci method names: resolveChangeScope" in str(exc),
        f"duplicate method rejection must name the shadowed method, got {exc}",
    )
else:
    require(False, "CI contract parser must reject duplicate class method shadowing")
require(
    "missing: secretsHistory" in strict_contract_message(
        ["codexAgentRoles", "deterministic", "secretsHistory", "advisories"],
        ["codexAgentRoles", "deterministic", "advisories"],
    ),
    "strict contract mismatch messages must name missing gates",
)

ci_contract = None

if dagger_source:
    try:
        ci_contract = extract_ci_contract(dagger_source)
    except ContractParseError as exc:
        errors.append(f"unable to parse Dagger source contract: {exc}")

check_methods = ci_contract["check_methods"] if ci_contract is not None else []
strict_calls = ci_contract["strict_calls"] if ci_contract is not None else []
strict_invocations = (
    ci_contract["strict_invocations"] if ci_contract is not None else []
)
strict_bindings = ci_contract["strict_bindings"] if ci_contract is not None else []
expected_strict_calls = [
    "resolveChangeScope",
    "codexAgentRoles",
    *check_methods,
    "advisoriesForScope",
]
expected_strict_invocations = [
    {"name": "resolveChangeScope", "arguments": "repo,base"},
    {"name": "codexAgentRoles", "arguments": "candidate"},
    {
        "name": "deterministic",
        "arguments": "candidate,policy,scope.runtime_required",
    },
    {"name": "secretsHistory", "arguments": "repo"},
    {
        "name": "advisoriesForScope",
        "arguments": "candidate,scope.runtime_required",
    },
]
expected_strict_bindings = [
    {"name": "repo", "expression": "source!"},
    {"name": "candidate", "expression": 'repo.withoutDirectory(".git")'},
    {
        "name": "policy",
        "expression": '(base??repo).withoutDirectory(".git")',
    },
    {
        "name": "scope",
        "expression": "awaitthis.resolveChangeScope(repo,base)",
    },
]
critical_control_plane_functions = [
    "ciContractContainer",
    "changeScopeContainer",
    "declaredRustCoverageMinimum",
    "rustFastContainer",
    "rustQualityContainer",
    "rustCoverageContainer",
    "rustAdvisoryContainer",
    "productionImageContainer",
    "productionImageService",
    "productionImageLoadRehearsalContainer",
    "productionImageIntegration",
    "resolveChangeScope",
    "deterministicForScope",
    "advisoriesForScope",
]
expected_control_plane_digests = {
    "ciContractContainer": {"3077600d7a8df2a0b30346ed8b0880fe6e862f62f58c4fd895b32d1f2294d69b"},
    "changeScopeContainer": {"ee20c16c0a92ce90b387501f79f0fe243c61cd27246e050f6a40fc0c2e6f4a85"},
    "declaredRustCoverageMinimum": {"8eece07e55b9a62e6ee57ec9f18b962d279df7c11709891edfa273c4289ae8c9"},
    "rustFastContainer": {"18fe68d967595e2209a79b389e25a6575075f7239a5fa9019a268c706dc0e672"},
    "rustQualityContainer": {"cabda5c56f8640c9d3b8b0128492c81bb722f05d917b2f81d19dff1108f2a825"},
    "rustCoverageContainer": {"96f465a1c41abbe1732d3866413741308a9d324a8b8ed3ece5d9ba718f712cd4"},
    "rustAdvisoryContainer": {"2382b5b3189cb3d9e51eed3ab044a03bf09b09120142fa94337d0c95a9b0973e"},
    "productionImageContainer": {"79fffb6298d0708883d3ed4b4c6396009592178f979b0fa935ea2c942d5711c9"},
    "productionImageService": {"0f96e41e8e1fe76ab17d2134e9eae04b74a6c052026c600dbdbe0efe38e5db89"},
    "productionImageLoadRehearsalContainer": {"49a49af86e8e9364e46f878538abb31df25dec84057d12959fa7b03910f28d33"},
    "productionImageIntegration": {"86260531fa90fedaf656def484c7b679be40df2f996704efee1ba78d06601b97"},
    "resolveChangeScope": {"b6e6b1291e7cc647b32d9e77e47691b8ae410fea48db52ae9a636f3c11f61258"},
    "deterministicForScope": {"30145b0c64c9e0c3f13fdff296deae480927da1bc7edaa750280a04056e9a7b8"},
    "advisoriesForScope": {"223268ff835764eff52a84df3e826f47931b8963324decf699360ae9d62f396d"},
}
actual_control_plane_digests = {}
for function_name in critical_control_plane_functions:
    try:
        actual_control_plane_digests[function_name] = body_digest(
            dagger_source, function_name
        )
    except ContractParseError as exc:
        errors.append(str(exc))
dagger_version_match = re.search(r'"engineVersion"\s*:\s*"v([^"]+)"', dagger_config)
required_dagger_version = (
    dagger_version_match.group(1) if dagger_version_match else None
)

require(
    check_methods,
    "dagger/src/index.ts must declare at least one @check gate",
)
require(
    strict_calls == expected_strict_calls,
    strict_contract_message(expected_strict_calls, strict_calls),
)
require(
    strict_invocations == expected_strict_invocations,
    "Ci.strict must thread the trusted candidate, policy, and runtime-required decision through every gate; "
    f"expected {expected_strict_invocations}, got {strict_invocations}",
)
require(
    strict_bindings == expected_strict_bindings,
    "Ci.strict must bind candidate and policy authority from source and the trusted base; "
    f"expected {expected_strict_bindings}, got {strict_bindings}",
)
for function_name, allowed_digests in expected_control_plane_digests.items():
    actual_digest = actual_control_plane_digests.get(function_name)
    require(
        actual_digest in allowed_digests,
        f"trusted CI control-plane body {function_name} drifted: expected one of {sorted(allowed_digests)}, got {actual_digest}",
    )
require(
    top_level_const_expression(dagger_source, "CARGO_LLVM_COV_VERSION")
    == '"0.8.7"'
    and top_level_const_expression(dagger_source, "RUST_COVERAGE_MIN_LINE_PCT")
    == "90.0",
    "trusted Rust coverage tool and line floor constants must remain pinned at 0.8.7 and 90.0",
)
require(
    required_dagger_version is not None,
    "dagger.json must define engineVersion",
)
require(
    source_argument_sync.is_file(),
    "dagger/scripts/sync_source_arguments.py must exist",
)
require(
    "async function cachePlatformKey()" in dagger_source,
    "dagger/src/index.ts must derive a cache platform key",
)
require(
    "await dag.defaultPlatform()" in dagger_source,
    "dagger/src/index.ts must scope cache volumes by dag.defaultPlatform()",
)
require(
    dagger_source.count("const platformKey = await cachePlatformKey()") == 2,
    "Each Dagger dependency container must compute a platform cache key once",
)
require(
    dagger_source.count("platformKey, imageKey, digest") == 3,
    "Every Dagger dependency cache volume must scope its key by platform, image, and lockfile digest",
)
require(
    "async function sourceTreeDigest(source: Directory)" in dagger_source
    and "return source.digest()" in dagger_source,
    "dagger/src/index.ts must derive a full source-tree digest for source-sensitive cache fallback",
)
require(
    'process.env.CANARY_DAGGER_CACHE_SCOPE?.trim()' in dagger_source
    and 'createHash("sha256").update(scope).digest("hex")' in dagger_source
    and "return sourceTreeDigest(source)" in dagger_source,
    "dagger/src/index.ts must prefer a hashed checkout cache scope and fall back to source-tree digest",
)
require(
    "const targetDigest = await rustTargetCacheDigest(source)" in dagger_source
    and "cacheVolumeName(targetNamespace, platformKey, imageKey, targetDigest)"
    in dagger_source,
    "Dagger must scope the Rust target cache by checkout/source identity to isolate concurrent divergent worktrees",
)
require(
    'CANARY_DAGGER_CACHE_SCOPE="${CANARY_DAGGER_CACHE_SCOPE:-$(cd "$ROOT" && pwd -P)}"' in dagger_wrapper
    and "export CANARY_DAGGER_CACHE_SCOPE" in dagger_wrapper,
    "bin/dagger must provide a physical checkout path cache scope for local Dagger invocations",
)
require(
    "async function rustContainer(" in dagger_source
    and ".withExec([\"cargo\", \"fmt\", \"--all\", \"--check\"])" in dagger_source
    and ".withExec([\"cargo\", \"check\", \"--workspace\", \"--all-targets\", \"--locked\"])" in dagger_source
    and "\"clippy\"" in dagger_source
    and ".withExec([\"cargo\", \"test\", \"--workspace\", \"--locked\"])" in dagger_source,
    "Dagger must run Rust format, check, clippy, and tests from a Rust container",
)

sync_result = subprocess.run(
    [sys.executable, str(source_argument_sync), "--check"],
    cwd=root,
    text=True,
    capture_output=True,
)
require(
    sync_result.returncode == 0,
    (
        "Dagger Directory arguments must stay in sync with "
        "dagger/scripts/sync_source_arguments.py"
        + (
            f": {(sync_result.stderr or sync_result.stdout).strip()}"
            if (sync_result.stderr or sync_result.stdout).strip()
            else ""
        )
    ),
)


with tempfile.TemporaryDirectory() as tmp:
    tmp_path = Path(tmp)
    shadow_path = tmp_path / "shadow"
    shadow_path.mkdir()
    log_path = tmp_path / "dagger.log"
    docker_log_path = tmp_path / "docker.log"
    ssh_log_path = tmp_path / "ssh.log"
    cargo_log_path = tmp_path / "cargo.log"
    npm_log_path = tmp_path / "npm.log"
    dagger_path = tmp_path / "dagger"
    dagger_path.write_text(
        "#!/usr/bin/env bash\n"
        "if [[ \"$1\" == \"version\" ]]; then\n"
        "  if [[ -n \"${DAGGER_STUB_VERSION+x}\" ]]; then\n"
        "    version=\"$DAGGER_STUB_VERSION\"\n"
        "  else\n"
        f"    version=\"{required_dagger_version}\"\n"
        "  fi\n"
        "  printf 'dagger v%s (image://registry.dagger.io/engine:v%s) darwin/arm64/v8\\n' \"$version\" \"$version\"\n"
        "  exit 0\n"
        "fi\n"
        f"printf '%s\\n' \"$*\" >> \"{log_path}\"\n"
        "if [[ \"$EXPECT_DOCKER_CALL\" == \"1\" ]]; then\n"
        "  docker version >/dev/null\n"
        "fi\n"
    )
    dagger_path.chmod(0o755)
    docker_path = tmp_path / "docker"
    docker_path.write_text(
        "#!/usr/bin/env bash\n"
        f"printf '%s\\n' \"$*\" >> \"{docker_log_path}\"\n"
        "if [[ -n \"$DOCKER_VERSION_DELAY_SECONDS\" ]]; then\n"
        "  sleep \"$DOCKER_VERSION_DELAY_SECONDS\"\n"
        "fi\n"
        "if [[ \"$DOCKER_VERSION_STATUS\" == \"fail\" ]]; then\n"
        "  exit 1\n"
        "fi\n"
    )
    docker_path.chmod(0o755)

    colima_dir = tmp_path / ".colima"
    colima_dir.mkdir()
    (colima_dir / "ssh_config").write_text("Host colima\n")
    colima_path = tmp_path / "colima"
    colima_path.write_text(
        "#!/usr/bin/env bash\n"
        "if [[ \"$1\" == \"version\" && \"$COLIMA_VERSION_STATUS\" != \"fail\" ]]; then\n"
        "  exit 0\n"
        "fi\n"
        "if [[ \"$1\" == \"status\" && \"$COLIMA_STATUS\" != \"fail\" ]]; then\n"
        "  exit 0\n"
        "fi\n"
        "exit 1\n"
    )
    colima_path.chmod(0o755)
    ssh_path = tmp_path / "ssh"
    ssh_path.write_text(
        "#!/usr/bin/env bash\n"
        f"printf '%s\\n' \"$*\" >> \"{ssh_log_path}\"\n"
    )
    ssh_path.chmod(0o755)
    cargo_path = tmp_path / "cargo"
    cargo_path.write_text(
        "#!/usr/bin/env bash\n"
        f"printf '%s\\n' \"$*\" >> \"{cargo_log_path}\"\n"
    )
    cargo_path.chmod(0o755)
    npm_path = tmp_path / "npm"
    npm_path.write_text(
        "#!/usr/bin/env bash\n"
        f"printf '%s\\n' \"$*\" >> \"{npm_log_path}\"\n"
    )
    npm_path.chmod(0o755)
    git_path = tmp_path / "git"
    git_path.write_text(
        "#!/usr/bin/env bash\n"
        "if [[ \"$*\" == *\"rev-parse --is-inside-work-tree\"* ]]; then\n"
        "  exit 1\n"
        "fi\n"
        "exit 0\n"
    )
    git_path.chmod(0o755)
    uname_path = tmp_path / "uname"
    uname_path.write_text(
        "#!/usr/bin/env bash\n"
        "if [[ \"$1\" == \"-s\" && -n \"$UNAME_OVERRIDE\" ]]; then\n"
        "  printf '%s\\n' \"$UNAME_OVERRIDE\"\n"
        "  exit 0\n"
        "fi\n"
        f"exec {real_uname} \"$@\"\n"
    )
    uname_path.chmod(0o755)

    env = os.environ.copy()
    env["HOME"] = tmp
    env["PATH"] = path_with_shims(shadow_path, tmp_path)

    def run(*command):
        return subprocess.run(
            [real_bash, *command],
            cwd=root,
            env=env,
            text=True,
            capture_output=True,
        )

    def read_lines(path):
        if not path.exists():
            return []
        return [line.strip() for line in path.read_text().splitlines() if line.strip()]

    def reset_logs():
        log_path.write_text("")
        docker_log_path.write_text("")
        ssh_log_path.write_text("")
        cargo_log_path.write_text("")
        npm_log_path.write_text("")

    reset_logs()
    result = run("bin/validate", "--fast")
    require(
        result.returncode == 0,
        "bin/validate --fast must succeed with a valid dagger binary on PATH",
    )
    require(
        read_lines(log_path) == ["call fast"],
        "bin/validate --fast must call dagger fast exactly once",
    )

    reset_logs()
    reset_shadow_commands(shadow_path)
    stale_version = "0.20.4"
    env["DAGGER_STUB_VERSION"] = stale_version
    result = run("bin/dagger", "call", "fast")
    require(
        result.returncode != 0,
        "bin/dagger must fail fast when the installed CLI version drifts from dagger.json",
    )
    require(
        f"Installed dagger CLI version v{stale_version} does not match repo-required version v{required_dagger_version}" in result.stderr,
        "bin/dagger must explain the pinned-version mismatch",
    )
    require(
        read_lines(log_path) == [],
        "bin/dagger must stop before delegating when the installed CLI version does not match dagger.json",
    )
    env.pop("DAGGER_STUB_VERSION", None)

    reset_logs()
    result = run("bin/validate", "--advisories")
    require(
        result.returncode == 0,
        "bin/validate --advisories must succeed with a valid dagger binary on PATH",
    )
    require(
        read_lines(log_path) == ["call advisories"],
        "bin/validate --advisories must call advisories exactly once",
    )

    reset_logs()
    result = run("bin/validate", "--strict")
    require(
        result.returncode == 0,
        "bin/validate --strict must succeed with a valid dagger binary on PATH",
    )
    require(
        read_lines(log_path) == ["call strict"],
        "bin/validate --strict must delegate to the strict Dagger entrypoint",
    )

    reset_logs()
    result = run("bin/validate")
    require(
        result.returncode == 0,
        "bin/validate must succeed with a valid dagger binary on PATH",
    )
    require(
        read_lines(log_path) == ["check"],
        "bin/validate without flags must delegate to dagger check exactly once",
    )

    reset_logs()
    result = run(".githooks/pre-commit")
    require(
        result.returncode == 0,
        "pre-commit hook must succeed with a valid dagger binary on PATH",
    )
    require(
        read_lines(log_path) == ["call fast"],
        "pre-commit hook must delegate to the fast validation path",
    )

    reset_logs()
    result = run(".githooks/pre-push")
    require(
        result.returncode == 0,
        "pre-push hook must succeed with a valid dagger binary on PATH",
    )
    require(
        read_lines(log_path) == ["call strict"],
        "pre-push hook must delegate to the strict validation path",
    )

    reset_logs()
    reset_shadow_commands(shadow_path)
    env["UNAME_OVERRIDE"] = "Linux"
    result = run("bin/dagger", "call", "fast")
    require(
        result.returncode == 0,
        "bin/dagger auto mode must stay direct on non-macOS hosts",
    )
    require(
        read_lines(log_path) == ["call fast"],
        "bin/dagger auto mode must still delegate to the installed dagger binary on non-macOS hosts",
    )
    require(
        read_lines(docker_log_path) == [],
        "bin/dagger auto mode must not probe Docker on non-macOS hosts",
    )
    require(
        read_lines(ssh_log_path) == [],
        "bin/dagger auto mode must not route through SSH on non-macOS hosts",
    )
    env.pop("UNAME_OVERRIDE", None)

    reset_logs()
    reset_shadow_commands(shadow_path)
    env["UNAME_OVERRIDE"] = "Darwin"
    result = run("bin/dagger", "call", "fast")
    require(
        result.returncode == 0,
        "bin/dagger auto mode must use the active Docker client on macOS when it is available",
    )
    require(
        read_lines(log_path) == ["call fast"],
        "bin/dagger auto mode must still delegate to the installed dagger binary",
    )
    require(
        read_lines(docker_log_path) == ["version"],
        "bin/dagger auto mode must probe the local Docker client before falling back",
    )
    require(
        read_lines(ssh_log_path) == [],
        "bin/dagger auto mode must not route through SSH when direct Docker access works",
    )
    env.pop("UNAME_OVERRIDE", None)

    reset_logs()
    reset_shadow_commands(shadow_path)
    env["UNAME_OVERRIDE"] = "Darwin"
    env["CANARY_DAGGER_DOCKER_TRANSPORT"] = "direct"
    result = run("bin/dagger", "call", "fast")
    require(
        result.returncode == 0,
        "bin/dagger must support the direct transport override",
    )
    require(
        read_lines(log_path) == ["call fast"],
        "bin/dagger direct transport must still delegate to the installed dagger binary",
    )
    require(
        read_lines(docker_log_path) == [],
        "bin/dagger direct transport must not probe the Docker client first",
    )
    require(
        read_lines(ssh_log_path) == [],
        "bin/dagger direct transport must not route through SSH",
    )
    env.pop("UNAME_OVERRIDE", None)
    env.pop("CANARY_DAGGER_DOCKER_TRANSPORT", None)

    reset_logs()
    reset_shadow_commands(shadow_path)
    env["UNAME_OVERRIDE"] = "Darwin"
    env["EXPECT_DOCKER_CALL"] = "1"
    shadow_missing_command(shadow_path, "docker")
    result = run("bin/dagger", "call", "fast")
    require(
        result.returncode == 0,
        "bin/dagger auto mode must fall back to Colima over SSH when the docker binary is unavailable",
    )
    require(
        read_lines(log_path) == ["call fast"],
        "bin/dagger auto mode must still delegate to the installed dagger binary when the docker binary is unavailable",
    )
    require(
        read_lines(docker_log_path) == [],
        "bin/dagger auto mode must skip the direct probe when the docker binary is unavailable",
    )
    require(
        read_lines(ssh_log_path) == [f"-F {colima_dir / 'ssh_config'} -T colima docker version"],
        "bin/dagger auto mode must route Docker calls through Colima over SSH when the docker binary is unavailable",
    )
    require(
        "Docker was unavailable; using Colima over SSH for repo-local Dagger." in result.stderr,
        "bin/dagger auto mode must announce when it falls back to Colima because Docker is unavailable",
    )
    reset_shadow_commands(shadow_path)
    env.pop("UNAME_OVERRIDE", None)
    env.pop("EXPECT_DOCKER_CALL", None)

    reset_logs()
    reset_shadow_commands(shadow_path)
    env["UNAME_OVERRIDE"] = "Darwin"
    shadow_missing_command(shadow_path, "docker")
    shadow_missing_command(shadow_path, "colima")
    result = run("bin/dagger", "call", "fast")
    require(
        result.returncode != 0,
        "bin/dagger auto mode must fail when neither direct Docker nor the Colima fallback is available",
    )
    require(
        "no Colima fallback is installed" in result.stderr,
        "bin/dagger auto mode must explain when the Colima fallback is unavailable",
    )
    reset_shadow_commands(shadow_path)
    env.pop("UNAME_OVERRIDE", None)

    reset_logs()
    reset_shadow_commands(shadow_path)
    env["UNAME_OVERRIDE"] = "Darwin"
    env["DOCKER_VERSION_STATUS"] = "fail"
    env["EXPECT_DOCKER_CALL"] = "1"
    result = run("bin/dagger", "call", "fast")
    require(
        result.returncode == 0,
        "bin/dagger auto mode must fall back to Colima over SSH on macOS when direct Docker access fails",
    )
    require(
        read_lines(log_path) == ["call fast"],
        "bin/dagger auto mode must still delegate to the installed dagger binary after the Colima fallback",
    )
    require(
        read_lines(docker_log_path) == ["version"],
        "bin/dagger auto mode must attempt the direct Docker probe before falling back",
    )
    require(
        read_lines(ssh_log_path) == [f"-F {colima_dir / 'ssh_config'} -T colima docker version"],
        "bin/dagger auto mode must route Docker calls through Colima over SSH after a failed direct probe",
    )
    require(
        "direct Docker access failed; using Colima over SSH for repo-local Dagger." in result.stderr,
        "bin/dagger auto mode must announce the Colima fallback after a failed direct Docker probe",
    )
    env.pop("UNAME_OVERRIDE", None)
    env.pop("DOCKER_VERSION_STATUS", None)
    env.pop("EXPECT_DOCKER_CALL", None)

    reset_logs()
    reset_shadow_commands(shadow_path)
    env["UNAME_OVERRIDE"] = "Darwin"
    env["COLIMA_STATUS"] = "fail"
    shadow_missing_command(shadow_path, "docker")
    result = run("bin/dagger", "call", "fast")
    require(
        result.returncode != 0,
        "bin/dagger auto mode must fail when the Colima fallback is installed but not running",
    )
    require(
        "Colima fallback is not running" in result.stderr,
        "bin/dagger auto mode must explain when the Colima fallback is installed but not running",
    )
    reset_shadow_commands(shadow_path)
    env.pop("UNAME_OVERRIDE", None)
    env.pop("COLIMA_STATUS", None)

    reset_logs()
    reset_shadow_commands(shadow_path)
    env["UNAME_OVERRIDE"] = "Darwin"
    env["DOCKER_VERSION_DELAY_SECONDS"] = "4"
    env["EXPECT_DOCKER_CALL"] = "1"
    result = run("bin/dagger", "call", "fast")
    require(
        result.returncode == 0,
        "bin/dagger auto mode must fall back to Colima over SSH when the direct Docker probe hangs",
    )
    require(
        read_lines(log_path) == ["call fast"],
        "bin/dagger auto mode must still delegate to the installed dagger binary after a hung direct probe",
    )
    require(
        read_lines(docker_log_path) == ["version"],
        "bin/dagger auto mode must attempt the direct Docker probe before timing out",
    )
    require(
        read_lines(ssh_log_path) == [f"-F {colima_dir / 'ssh_config'} -T colima docker version"],
        "bin/dagger auto mode must route Docker calls through Colima over SSH after a hung direct probe",
    )
    require(
        "direct Docker probe timed out; using Colima over SSH for repo-local Dagger." in result.stderr,
        "bin/dagger auto mode must announce the Colima fallback after a timed-out direct Docker probe",
    )
    env.pop("UNAME_OVERRIDE", None)
    env.pop("DOCKER_VERSION_DELAY_SECONDS", None)
    env.pop("EXPECT_DOCKER_CALL", None)

    reset_logs()
    reset_shadow_commands(shadow_path)
    env["UNAME_OVERRIDE"] = "Darwin"
    env["DOCKER_VERSION_DELAY_SECONDS"] = "2"
    env["CANARY_DOCKER_PROBE_TIMEOUT_SECONDS"] = "1"
    env["EXPECT_DOCKER_CALL"] = "1"
    result = run("bin/dagger", "call", "fast")
    require(
        result.returncode == 0,
        "bin/dagger auto mode must honor the configurable Docker probe timeout when deciding to fall back",
    )
    require(
        read_lines(log_path) == ["call fast"],
        "bin/dagger auto mode must still delegate after a timeout-driven Colima fallback",
    )
    require(
        read_lines(docker_log_path) == ["version"],
        "bin/dagger auto mode must still attempt the direct Docker probe before applying a custom timeout",
    )
    require(
        read_lines(ssh_log_path) == [f"-F {colima_dir / 'ssh_config'} -T colima docker version"],
        "bin/dagger auto mode must route through Colima when the configured Docker probe timeout is exceeded",
    )
    require(
        "direct Docker probe timed out; using Colima over SSH for repo-local Dagger." in result.stderr,
        "bin/dagger auto mode must report timeout-driven Colima fallback when the probe timeout is configured",
    )
    env.pop("UNAME_OVERRIDE", None)
    env.pop("DOCKER_VERSION_DELAY_SECONDS", None)
    env.pop("CANARY_DOCKER_PROBE_TIMEOUT_SECONDS", None)
    env.pop("EXPECT_DOCKER_CALL", None)

    reset_logs()
    reset_shadow_commands(shadow_path)
    env["CANARY_DAGGER_DOCKER_TRANSPORT"] = "colima-ssh"
    env["EXPECT_DOCKER_CALL"] = "1"
    result = run("bin/dagger", "call", "fast")
    require(
        result.returncode == 0,
        "bin/dagger must support the Colima transport override",
    )
    require(
        read_lines(log_path) == ["call fast"],
        "bin/dagger must still delegate to the installed dagger binary under the Colima transport override",
    )
    require(
        read_lines(docker_log_path) == [],
        "bin/dagger must not probe the direct Docker client when Colima transport is forced",
    )
    require(
        read_lines(ssh_log_path) == [f"-F {colima_dir / 'ssh_config'} -T colima docker version"],
        "bin/dagger must route Docker calls through Colima over SSH",
    )
    env.pop("CANARY_DAGGER_DOCKER_TRANSPORT", None)
    env.pop("EXPECT_DOCKER_CALL", None)

    reset_logs()
    reset_shadow_commands(shadow_path)
    env["UNAME_OVERRIDE"] = "Darwin"
    result = run("bin/bootstrap")
    require(
        result.returncode == 0,
        "bin/bootstrap must succeed with stubbed package managers and a valid dagger binary on PATH",
    )
    require(
        "==> tooling:" not in result.stdout,
        "bin/bootstrap must stay quiet about Docker runtimes when the active Docker client works",
    )
    require(
        read_lines(cargo_log_path) == ["fetch --locked"],
        "bin/bootstrap must fetch locked Rust dependencies",
    )
    require(
        read_lines(npm_log_path) == ["ci"],
        "bin/bootstrap must run npm ci for the TypeScript SDK",
    )
    env.pop("UNAME_OVERRIDE", None)

    reset_logs()
    reset_shadow_commands(shadow_path)
    env["UNAME_OVERRIDE"] = "Darwin"
    shadow_missing_command(shadow_path, "docker")
    shadow_missing_command(shadow_path, "colima")
    result = run("bin/bootstrap")
    require(
        result.returncode == 0,
        "bin/bootstrap must succeed when Docker and Colima are both unavailable",
    )
    require(
        "macOS local validation needs a working Docker runtime" in result.stdout,
        "bin/bootstrap must explain how to restore local validation when no Docker runtime is available on macOS",
    )
    reset_shadow_commands(shadow_path)
    env.pop("UNAME_OVERRIDE", None)

    reset_logs()
    reset_shadow_commands(shadow_path)
    env["UNAME_OVERRIDE"] = "Darwin"
    env["COLIMA_STATUS"] = "fail"
    shadow_missing_command(shadow_path, "docker")
    result = run("bin/bootstrap")
    require(
        result.returncode == 0,
        "bin/bootstrap must succeed when Colima is installed but not running",
    )
    require(
        "no working Docker runtime detected" in result.stdout,
        "bin/bootstrap must direct Colima users to start Colima when Docker probing fails on macOS",
    )
    reset_shadow_commands(shadow_path)
    env.pop("UNAME_OVERRIDE", None)
    env.pop("COLIMA_STATUS", None)

require(
    "steps.dagger_version.outputs.version" in workflow,
    "GitHub workflow must source the Dagger version from dagger.json",
)
require(
    "pull_request_target:" in workflow and "\npull_request:" not in workflow,
    "GitHub workflow must enforce pull requests from the base-branch context with pull_request_target",
)
require(
    re.search(
        r"group:\s+ci-\$\{\{\s*github\.workflow\s*\}\}-\$\{\{\s*github\.event\.pull_request\.number\s*\|\|\s*github\.ref\s*\}\}",
        workflow,
    ),
    "GitHub workflow concurrency must isolate pull requests by number instead of collapsing them onto the base ref",
)
require(
    re.search(
        r"name:\s+Checkout trusted CI control plane[\s\S]*?uses:\s+actions/checkout@[\s\S]*?path:\s+\.ci/trusted[\s\S]*?ref:\s+\$\{\{\s*github\.event\.pull_request\.base\.sha\s*\|\|\s*github\.sha\s*\}\}[\s\S]*?persist-credentials:\s+false",
        workflow,
    ),
    "GitHub workflow must checkout the trusted CI control plane from the base SHA without persisting credentials",
)
require(
    re.search(
        r"name:\s+Checkout candidate source[\s\S]*?uses:\s+actions/checkout@[\s\S]*?path:\s+\.ci/candidate[\s\S]*?repository:\s+\$\{\{\s*github\.event\.pull_request\.head\.repo\.full_name\s*\|\|\s*github\.repository\s*\}\}[\s\S]*?ref:\s+\$\{\{\s*github\.event\.pull_request\.head\.sha\s*\|\|\s*github\.sha\s*\}\}[\s\S]*?persist-credentials:\s+false",
        workflow,
    ),
    "GitHub workflow must checkout the candidate source separately from the trusted control plane without persisting credentials",
)
require(
    re.search(
        r"name:\s+Read trusted Dagger engine version[\s\S]*?\.ci/trusted/dagger\.json",
        workflow,
    ),
    "GitHub workflow must read the Dagger engine version from the trusted control-plane checkout",
)
require(
    re.search(
        r"name:\s+Run trusted Dagger strict CI with scoped base[\s\S]*?if:\s+github\.event_name\s+==\s+'pull_request_target'[\s\S]*?uses:\s+dagger/dagger-for-github@[\s\S]*?workdir:\s+\.ci/trusted[\s\S]*?verb:\s+call[\s\S]*?args:\s+strict\s+--source=\.\./candidate\s+--base=\.\./trusted",
        workflow,
    ),
    "Pull-request CI must run trusted strict against the candidate and pass the trusted base for fail-closed scope classification",
)
require(
    "Run Dagger codex role validation" not in workflow,
    "GitHub workflow must not duplicate codex-agent-roles outside the strict Dagger entrypoint",
)
require(
    "Run Dagger advisories" not in workflow,
    "GitHub workflow must not duplicate advisories outside the strict Dagger entrypoint",
)
require(
    'CANARY_WITNESS_MAX_LATENCY_MS: "2000"' in uptime_workflow,
    "The external witness workflow must enforce the post-deploy latency ceiling",
)

if not errors:
    sys.exit(0)

for error in errors:
    print(error, file=sys.stderr)

sys.exit(1)
