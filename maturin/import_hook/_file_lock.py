import contextlib
import errno
import os
import platform
import time
from abc import ABC, abstractmethod
from pathlib import Path
from types import ModuleType, TracebackType
from typing import Optional, Type

from maturin.import_hook._logging import logger

fcntl: Optional[ModuleType] = None
with contextlib.suppress(ImportError):
    import fcntl


msvcrt: Optional[ModuleType] = None
with contextlib.suppress(ImportError):
    import msvcrt


class LockError(Exception):
    pass


class FileLock(ABC):
    def __init__(
        self, path: Path, timeout_seconds: Optional[float], poll_interval: float = 0.05
    ) -> None:
        self._path = path
        self._timeout_seconds = timeout_seconds
        self._poll_interval = poll_interval
        self._is_locked = False

    @property
    def is_locked(self) -> bool:
        return self._is_locked

    def acquire(self) -> None:
        if self._is_locked:
            msg = f"{type(self).__name__} is not reentrant"
            raise LockError(msg)
        start = time.time()
        first_attempt = True
        while True:
            self.try_acquire()
            if self._is_locked:
                return
            if first_attempt:
                logger.info("waiting on lock %s (%s)", self._path, type(self).__name__)
                first_attempt = False

            if (
                self._timeout_seconds is not None
                and time.time() - start > self._timeout_seconds
            ):
                msg = f"failed to acquire lock {self._path} in time"
                raise TimeoutError(msg)
            else:
                time.sleep(self._poll_interval)

    @abstractmethod
    def try_acquire(self) -> None:
        raise NotImplementedError

    @abstractmethod
    def release(self) -> None:
        raise NotImplementedError

    def __enter__(self) -> None:
        self.acquire()

    def __exit__(
        self,
        exc_type: Optional[Type[BaseException]],
        exc_val: Optional[BaseException],
        exc_tb: Optional[TracebackType],
    ) -> None:
        self.release()

    def __del__(self) -> None:
        self.release()

    @staticmethod
    def new(path: Path, timeout_seconds: Optional[float]) -> "FileLock":
        if os.name == "posix":
            if fcntl is None:
                return AtomicOpenLock(path, timeout_seconds)
            else:
                return FcntlFileLock(path, timeout_seconds)
        elif platform.platform().lower() == "windows":
            return WindowsFileLock(path, timeout_seconds)
        else:
            return AtomicOpenLock(path, timeout_seconds)


class FcntlFileLock(FileLock):
    def __init__(self, path: Path, timeout_seconds: Optional[float]) -> None:
        super().__init__(path, timeout_seconds)
        self._path.parent.mkdir(parents=True, exist_ok=True)
        self._fd = os.open(self._path, os.O_WRONLY | os.O_CREAT)

    def __del__(self) -> None:
        self.release()
        os.close(self._fd)

    def try_acquire(self) -> None:
        if self._is_locked:
            return
        assert fcntl is not None
        try:
            fcntl.flock(self._fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError as e:
            if e.errno == errno.ENOSYS:
                msg = "flock not supported by filesystem"
                raise LockError(msg)
        else:
            self._is_locked = True

    def release(self) -> None:
        if self._is_locked:
            assert fcntl is not None
            # do not remove the lock file to avoid a potential race condition where another
            # process opens the file then the file gets unlinked, leaving that process with
            # a handle to a dangling file, leading it to believe it holds the lock when it doesn't
            fcntl.flock(self._fd, fcntl.LOCK_UN)
            self._is_locked = False


class WindowsFileLock(FileLock):
    def __init__(self, path: Path, timeout_seconds: Optional[float]) -> None:
        super().__init__(path, timeout_seconds)
        self._fd = os.open(self._path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC)

    def try_acquire(self) -> None:
        if self._is_locked:
            return
        assert msvcrt is not None
        try:
            msvcrt.locking(self._fd, msvcrt.LK_NBLCK, 1)
        except OSError as e:
            if e.errno != errno.EACCES:
                msg = f"failed to acquire lock: {e}"
                raise LockError(msg)
        else:
            self._is_locked = True

    def release(self) -> None:
        if self._is_locked:
            assert msvcrt is not None
            msvcrt.locking(self._fd, msvcrt.LK_UNLCK, 1)
            self._is_locked = False


class AtomicOpenLock(FileLock):
    """This lock should be supported on all platforms but is not as reliable as it depends
    on the filesystem supporting atomic file creation [1].


    - [1] https://man7.org/linux/man-pages/man2/open.2.html
    """

    def __init__(self, path: Path, timeout_seconds: Optional[float]) -> None:
        super().__init__(path, timeout_seconds)
        self._fd: Optional[int] = None
        self._is_windows = platform.platform().lower() == "windows"

    def try_acquire(self) -> None:
        if self._is_locked:
            return
        assert self._fd is None
        try:
            fd = os.open(self._path, os.O_WRONLY | os.O_CREAT | os.O_EXCL)
        except OSError as e:
            if not (
                e.errno == errno.EEXIST
                or (self._is_windows and e.errno == errno.EACCES)
            ):
                msg = f"failed to acquire lock: {e}"
                raise LockError(msg)
        else:
            self._fd = fd
            self._is_locked = True

    def release(self) -> None:
        if self._is_locked:
            assert self._fd is not None
            os.close(self._fd)
            self._fd = None
            self._is_locked = False
            self._path.unlink(missing_ok=True)
