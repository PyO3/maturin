#!/usr/bin/env python3
import re
import subprocess
from pathlib import Path


FILES = [
    "README.md",
    "guide/src/develop.md",
    "guide/src/tutorial.md",
    "guide/src/distribution.md",
]


def main():
    root = Path(
        subprocess.check_output(
            ["git", "rev-parse", "--show-toplevel"], text=True
        ).strip()
    )

    for path in FILES:
        content = root.joinpath(path).read_text()

        matcher = re.compile(
            r"```\nUSAGE:\n    maturin (\w+) (.*?)```", re.MULTILINE | re.DOTALL
        )

        replaces = {}
        for command, old in matcher.findall(content):
            command_output = subprocess.check_output(
                ["cargo", "run", "--", command.lower(), "--help"], text=True
            )
            new = "USAGE:" + command_output.strip().split("USAGE:")[1] + "\n"
            # Remove trailing whitespace
            new = re.sub(" +\n", "\n", new)
            old = "USAGE:\n    maturin " + command + " " + old
            replaces[old] = new

        for old, new in replaces.items():
            content = content.replace(old, new)
        root.joinpath(path).write_text(content)


if __name__ == "__main__":
    main()
