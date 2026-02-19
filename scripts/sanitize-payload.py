#!/usr/bin/env python3
"""
Sanitize webhook payloads before forwarding to LLM-backed agents (e.g. OpenClaw).

Threat model:
  Attackers embed prompt injection in user-controlled fields (PR titles,
  descriptions, comments, branch names, Linear issue bodies) that flow into
  agent prompts and hijack behavior.

Defense layers:
  1. Allowlist extraction — only forward known-safe structural fields
  2. Text field fencing — wrap user content in delimiters so the LLM treats
     it as quoted data, not instructions
  3. Pattern flagging — detect known injection patterns and flag for review
  4. Size limits — truncate oversized fields that could be used for
     context-stuffing attacks

Usage:
  # Pipe entire payload, get sanitized JSON on stdout
  echo "$GITHUB_PAYLOAD" | sanitize-payload.py --source github

  # Or from a file
  sanitize-payload.py --source linear --input payload.json

  # Flag-only mode (exit 1 if suspicious, don't modify)
  echo "$PAYLOAD" | sanitize-payload.py --source github --flag-only

Exit codes:
  0 — clean (or sanitized output written)
  1 — flagged as suspicious (--flag-only mode)
  2 — invalid input / parse error
"""

import argparse
import json
import re
import sys
from typing import Any

# --- Size limits ---
MAX_TITLE_LEN = 500
MAX_BODY_LEN = 50_000
MAX_COMMENT_LEN = 20_000
MAX_BRANCH_LEN = 200

# --- Injection patterns ---
# These catch common prompt injection techniques. Not exhaustive — defense in
# depth means the fencing layer matters more than pattern matching.
INJECTION_PATTERNS = [
    # Direct role/instruction hijacking
    r"(?i)\b(you are|you're) (now |)(a |an |)(new |different |)?(assistant|ai|bot|system|admin)\b",
    r"(?i)\bignore (all |)(previous|prior|above|earlier) (instructions|prompts|context|rules)\b",
    r"(?i)\bignore (everything|anything) (above|before|previously)\b",
    r"(?i)\bforget (your|all|previous|prior) (instructions|rules|prompts|constraints)\b",
    r"(?i)\boverride (system|safety|security) (prompt|instructions|rules|settings)\b",
    r"(?i)\b(system|admin|root) ?(prompt|override|mode|access)\b",
    r"(?i)\bnew (system ?prompt|instructions|persona|role)\b",
    # Delimiter escape attempts
    r"(?i)<\/?system>",
    r"(?i)\[INST\]",
    r"(?i)\[\/INST\]",
    r"(?i)<<SYS>>",
    r"(?i)<\|im_start\|>",
    r"(?i)```system",
    # Exfiltration / action hijacking
    r"(?i)\b(execute|run|eval|exec)\s*\(",
    r"(?i)\bcurl\s+-",
    r"(?i)\bwget\s+",
    r"(?i)\b(rm|del|remove)\s+(-rf?|--force)",
    # Encoded payloads
    r"(?i)\bbase64[_\s\-]*(decode|encode|eval)",
    r"(?i)\batob\s*\(",
    # Social engineering the agent
    r"(?i)\bdo not (review|check|flag|report|mention)\b",
    r"(?i)\bthis is (a |)(test|safe|authorized|harmless)\b.*\b(ignore|skip|bypass)\b",
    r"(?i)\bpretend (you|that|to)\b",
    r"(?i)\brole\s*:\s*(system|assistant|user)\b",
]

COMPILED_PATTERNS = [re.compile(p) for p in INJECTION_PATTERNS]


def detect_injections(text: str) -> list[str]:
    """Return list of matched pattern descriptions."""
    if not text:
        return []
    hits = []
    for pattern in COMPILED_PATTERNS:
        match = pattern.search(text)
        if match:
            hits.append(f"pattern={pattern.pattern!r} matched={match.group()!r}")
    return hits


def fence(text: str, label: str) -> str:
    """Wrap user-controlled text in clear data delimiters.

    This tells the LLM "everything between these markers is untrusted user
    content to be processed as data, not as instructions."
    """
    if not text:
        return ""
    # Use a delimiter unlikely to appear in normal content
    boundary = f"--- BEGIN UNTRUSTED {label.upper()} ---"
    end = f"--- END UNTRUSTED {label.upper()} ---"
    return f"{boundary}\n{text}\n{end}"


def truncate(text: str, max_len: int) -> str:
    if not text or len(text) <= max_len:
        return text or ""
    return text[:max_len] + f"\n[TRUNCATED: original was {len(text)} chars]"


# --- GitHub payload extraction ---

GITHUB_ALLOWLIST = {
    "action", "number", "sender", "repository", "pull_request",
    "review", "comment", "issue",
}

def sanitize_github(payload: dict) -> dict:
    """Extract and sanitize GitHub webhook payload."""
    out: dict[str, Any] = {}

    # Structural fields (safe — controlled by GitHub, not users)
    out["action"] = payload.get("action", "")
    out["number"] = payload.get("number") or payload.get("pull_request", {}).get("number")

    sender = payload.get("sender", {})
    out["sender"] = {"login": sender.get("login", "")}

    repo = payload.get("repository", {})
    out["repository"] = {
        "full_name": repo.get("full_name", ""),
        "default_branch": repo.get("default_branch", ""),
    }

    # GitHub App installation ID (safe — controlled by GitHub)
    installation = payload.get("installation", {})
    if installation:
        out["installation"] = {"id": installation.get("id")}

    # PR fields — user-controlled text gets fenced
    pr = payload.get("pull_request", {})
    if pr:
        head = pr.get("head", {})
        base = pr.get("base", {})
        out["pull_request"] = {
            "number": pr.get("number"),
            "state": pr.get("state", ""),
            "draft": pr.get("draft", False),
            "merged": pr.get("merged", False),
            "title": fence(truncate(pr.get("title", ""), MAX_TITLE_LEN), "pr title"),
            "body": fence(truncate(pr.get("body", ""), MAX_BODY_LEN), "pr body"),
            "head": {
                "ref": truncate(head.get("ref", ""), MAX_BRANCH_LEN),
                "sha": head.get("sha", ""),
            },
            "base": {
                "ref": truncate(base.get("ref", ""), MAX_BRANCH_LEN),
                "sha": base.get("sha", ""),
            },
            "user": {"login": pr.get("user", {}).get("login", "")},
            "changed_files": pr.get("changed_files"),
            "additions": pr.get("additions"),
            "deletions": pr.get("deletions"),
        }

    # Review
    review = payload.get("review", {})
    if review:
        out["review"] = {
            "state": review.get("state", ""),
            "body": fence(truncate(review.get("body", ""), MAX_COMMENT_LEN), "review body"),
            "user": {"login": review.get("user", {}).get("login", "")},
        }

    # Comment
    comment = payload.get("comment", {})
    if comment:
        out["comment"] = {
            "id": comment.get("id"),
            "body": fence(truncate(comment.get("body", ""), MAX_COMMENT_LEN), "comment body"),
            "user": {"login": comment.get("user", {}).get("login", "")},
            "path": comment.get("path", ""),
            "line": comment.get("line"),
        }

    return out


# --- Linear payload extraction ---

def sanitize_linear(payload: dict) -> dict:
    """Extract and sanitize Linear webhook payload."""
    out: dict[str, Any] = {}

    # Structural
    out["type"] = payload.get("type", "")
    out["action"] = payload.get("action", "")
    out["url"] = payload.get("url", "")

    data = payload.get("data", {})
    if not data:
        return out

    out["data"] = {
        "id": data.get("id", ""),
        "identifier": data.get("identifier", ""),
        "state": data.get("state", {}),
        "priority": data.get("priority"),
        "team": {"key": data.get("team", {}).get("key", "")},
        "assignee": {"name": (data.get("assignee") or {}).get("name", "")},
        "labels": [{"name": l.get("name", "")} for l in (data.get("labels") or [])],
    }

    # User-controlled text — fence it
    if data.get("title"):
        out["data"]["title"] = fence(
            truncate(data["title"], MAX_TITLE_LEN), "issue title"
        )
    if data.get("description"):
        out["data"]["description"] = fence(
            truncate(data["description"], MAX_BODY_LEN), "issue description"
        )
    # Comment body (for Comment events, body is in data directly)
    if data.get("body"):
        out["data"]["body"] = fence(
            truncate(data["body"], MAX_COMMENT_LEN), "comment body"
        )

    return out


def main():
    parser = argparse.ArgumentParser(description="Sanitize webhook payloads for LLM agents")
    parser.add_argument("--source", required=True, choices=["github", "linear"])
    parser.add_argument("--input", help="Input file (default: stdin)")
    parser.add_argument("--flag-only", action="store_true",
                        help="Only check for injections, don't sanitize. Exit 1 if suspicious.")
    parser.add_argument("--verbose", action="store_true",
                        help="Print detection details to stderr")
    args = parser.parse_args()

    # Read payload
    try:
        if args.input:
            with open(args.input) as f:
                payload = json.load(f)
        else:
            payload = json.load(sys.stdin)
    except (json.JSONDecodeError, FileNotFoundError) as e:
        print(f"error: {e}", file=sys.stderr)
        sys.exit(2)

    # Scan all string values for injection patterns
    all_text_fields = extract_all_strings(payload)
    all_hits = []
    for field_path, text in all_text_fields:
        hits = detect_injections(text)
        if hits:
            all_hits.append((field_path, hits))

    if all_hits:
        for field_path, hits in all_hits:
            for h in hits:
                print(f"[FLAGGED] {field_path}: {h}", file=sys.stderr)

    if args.flag_only:
        if all_hits:
            sys.exit(1)
        else:
            sys.exit(0)

    # Sanitize
    if args.source == "github":
        sanitized = sanitize_github(payload)
    else:
        sanitized = sanitize_linear(payload)

    # Attach metadata
    sanitized["_sanitized"] = True
    if all_hits:
        sanitized["_flags"] = [
            {"field": fp, "count": len(hits)}
            for fp, hits in all_hits
        ]

    json.dump(sanitized, sys.stdout, indent=None, ensure_ascii=False)
    sys.stdout.write("\n")


def extract_all_strings(obj: Any, path: str = "") -> list[tuple[str, str]]:
    """Recursively extract all string values with their dotted paths."""
    results = []
    if isinstance(obj, str):
        if len(obj) > 10:  # skip tiny structural values
            results.append((path, obj))
    elif isinstance(obj, dict):
        for k, v in obj.items():
            results.extend(extract_all_strings(v, f"{path}.{k}" if path else k))
    elif isinstance(obj, list):
        for i, v in enumerate(obj):
            results.extend(extract_all_strings(v, f"{path}.{i}"))
    return results


if __name__ == "__main__":
    main()
