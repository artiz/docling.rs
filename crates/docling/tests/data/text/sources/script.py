#!/usr/bin/env python3
"""A tiny script: the `#` lines must not become headings."""

import sys


def main(argv):
    # Print the arguments, one per line.
    for arg in argv[1:]:
        print(f"* {arg}")


if __name__ == "__main__":
    main(sys.argv)
