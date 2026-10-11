"""Stand-in for the reverse-architect binary in tests (no Rust build needed).

Mirrors the real CLI surface the runner uses: `--version`, and the bare
generate form `<tree> --out <dir> [options...]`. It writes a small,
deterministic corpus derived from the tree so a changed tree changes it.
`--fail` makes it exit non-zero, to test error handling.
"""
import sys
from pathlib import Path


def main(argv: list[str]) -> int:
    # `--ra-version <name>` ahead of everything else plays another version of RA:
    # it names itself in `--version` and leaves a marker file in its corpus.
    label = None
    if argv[:1] == ["--ra-version"]:
        label, argv = argv[1], argv[2:]
    if argv == ["--version"]:
        print(f"reverse-architect {label or '0.0.0-fake'}")
        return 0
    if "--fail" in argv:
        print("fake ra: simulated failure", file=sys.stderr)
        return 3
    tree = Path(argv[0])
    out = Path(argv[argv.index("--out") + 1])
    options = [a for i, a in enumerate(argv[1:], start=1)
               if a != "--out" and argv[i - 1] != "--out"]
    files = sorted(p.relative_to(tree).as_posix() for p in tree.rglob("*") if p.is_file())
    (out / "cards").mkdir(parents=True, exist_ok=True)
    (out / "index.md").write_text("# Index\n" + "".join(f"- {f}\n" for f in files),
                                  encoding="utf-8", newline="\n")
    for f in files:
        name = f.replace("/", "__") + ".md"
        (out / "cards" / name).write_text(f"# {f}\n", encoding="utf-8", newline="\n")
    if label is not None:
        (out / "ra-version.txt").write_text(label + "\n", encoding="utf-8", newline="\n")
    (out / "options.txt").write_text(" ".join(options) + "\n", encoding="utf-8", newline="\n")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
