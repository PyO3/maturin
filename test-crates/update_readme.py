#!/usr/bin/env python3
import re
import subprocess
from pathlib import Path


def main():
    root = Path(
        subprocess.check_output(
            ["git", "rev-parse", "--show-toplevel"], text=True
        ).strip()
    )

    readme = root.joinpath("Readme.md").read_text()

    matcher = re.compile(r"### (\w+)\n\n```\n(USAGE:.*?)```", re.MULTILINE | re.DOTALL)

    replaces = {}
    for command, old in matcher.findall(readme):
        command_output = subprocess.check_output(
            ["cargo", "run", "--", command.lower(), "--help"], text=True
        )
        new = "USAGE:" + command_output.strip().split("USAGE:")[1] + "\n"
        # Remove trailing whitespace
        new = re.sub(" +\n", "\n", new)
        replaces[old] = new

    for old, new in replaces.items():
        readme = readme.replace(old, new)
    root.joinpath("Readme.md").write_text(readme)


if __name__ == "__main__":
    main()
