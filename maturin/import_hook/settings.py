from abc import ABC, abstractmethod
from dataclasses import dataclass
from pathlib import Path
from typing import Dict, List, Optional, Set

__all__ = [
    "MaturinSettings",
    "MaturinBuildSettings",
    "MaturinDevelopSettings",
    "MaturinSettingsProvider",
]


@dataclass
class MaturinSettings:
    """Settings common to `maturin build` and `maturin develop`."""

    release: bool = False
    strip: bool = False
    quiet: bool = False
    jobs: Optional[int] = None
    profile: Optional[str] = None
    features: Optional[List[str]] = None
    all_features: bool = False
    no_default_features: bool = False
    target: Optional[str] = None
    ignore_rust_version: bool = False
    color: Optional[bool] = None
    frozen: bool = False
    locked: bool = False
    offline: bool = False
    config: Optional[Dict[str, str]] = None
    unstable_flags: Optional[List[str]] = None
    verbose: int = 0
    rustc_flags: Optional[List[str]] = None

    @staticmethod
    def supported_commands() -> Set[str]:
        return {"build", "develop"}

    @staticmethod
    def default() -> "MaturinSettings":
        """MaturinSettings() sets no flags but default() corresponds to some sensible defaults."""
        return MaturinSettings(
            color=True,
        )

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
        if self.profile is not None:
            args.append("--profile")
            args.append(self.profile)
        if self.features:
            args.append("--features")
            args.append(",".join(self.features))
        if self.all_features:
            args.append("--all-features")
        if self.no_default_features:
            args.append("--no-default-features")
        if self.target is not None:
            args.append("--target")
            args.append(self.target)
        if self.ignore_rust_version:
            args.append("--ignore-rust-version")
        if self.color is not None:
            args.append("--color")
            if self.color:
                args.append("always")
            else:
                args.append("never")
        if self.frozen:
            args.append("--frozen")
        if self.locked:
            args.append("--locked")
        if self.offline:
            args.append("--offline")
        if self.config is not None:
            for key, value in self.config.items():
                args.append("--config")
                args.append(f"{key}={value}")
        if self.unstable_flags is not None:
            for flag in self.unstable_flags:
                args.append("-Z")
                args.append(flag)
        if self.verbose > 0:
            args.append("-{}".format("v" * self.verbose))
        if self.rustc_flags is not None:
            args.extend(self.rustc_flags)
        return args


@dataclass
class MaturinBuildSettings(MaturinSettings):
    """settings for `maturin build`."""

    skip_auditwheel: bool = False
    zig: bool = False

    @staticmethod
    def supported_commands() -> Set[str]:
        return {"build"}

    def to_args(self) -> List[str]:
        args = []
        if self.skip_auditwheel:
            args.append("--skip-auditwheel")
        if self.zig:
            args.append("--zig")
        args.extend(super().to_args())
        return args


@dataclass
class MaturinDevelopSettings(MaturinSettings):
    """settings for `maturin develop`."""

    extras: Optional[List[str]] = None
    skip_install: bool = False

    @staticmethod
    def supported_commands() -> Set[str]:
        return {"develop"}

    def to_args(self) -> List[str]:
        args = []
        if self.extras is not None:
            args.append("--extras")
            args.append(",".join(self.extras))
        if self.skip_install:
            args.append("--skip-install")
        args.extend(super().to_args())
        return args


class MaturinSettingsProvider(ABC):
    @abstractmethod
    def get_settings(self, module_path: str, source_path: Path) -> MaturinSettings:
        raise NotImplementedError
