#!/usr/bin/env bash
for d in ./*; do
  if [ -d "$d" ]; then
    if [ -f "$d/Cargo.lock" ]; then
      cargo update --manifest-path "$d/Cargo.toml"
      echo "$d updated"
    fi
  fi
done
