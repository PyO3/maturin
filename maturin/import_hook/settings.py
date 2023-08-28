from abc import ABC, abstractmethod
from dataclasses import dataclass
from pathlib import Path
from typing import List, Optional

__all__ = ["MaturinSettings", "MaturinSettingsProvider"]


@dataclass
class MaturinSettings:
    release: bool = False
    strip: bool = False
    quiet: bool = False
    jobs: Optional[int] = None
    features: Optional[List[str]] = None
    all_features: bool = False
    no_default_features: bool = False
    frozen: bool = False
    locked: bool = False
    offline: bool = False
    verbose: int = 0

    def __post_init__(self) -> None:
        if self.verbose not in (0, 1, 2):
            msg = f"invalid verbose value: {self.verbose}"
            raise ValueError(msg)

    def to_args(self) -> List[str]:
        args = []
        if self.release:
            args.append("--release")
        if self.strip:
            args.append("--strip")
        if self.quiet:
            args.append("--quiet")
        if self.jobs is not None:
            args.append("--jobs")
            args.append(str(self.jobs))
        if self.features:
            args.append("--features")
            args.append(",".join(self.features))
        if self.all_features:
            args.append("--all-features")
        if self.no_default_features:
            args.append("--no-default-features")
        if self.frozen:
            args.append("--frozen")
        if self.locked:
            args.append("--locked")
        if self.offline:
            args.append("--offline")
        if self.verbose > 0:
            args.append("-{}".format("v" * self.verbose))
        return args


class MaturinSettingsProvider(ABC):
    @abstractmethod
    def get_settings(self, module_path: str, source_path: Path) -> MaturinSettings:
        raise NotImplementedError
