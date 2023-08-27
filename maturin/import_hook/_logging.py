import logging

logger = logging.getLogger("maturin.import_hook")


class _LevelDependentFormatter(logging.Formatter):
    def __init__(self) -> None:
        super().__init__(fmt="", datefmt=None, style="%")
        self._info_fmt = "%(message)s"
        self._other_fmt = "%(name)s [%(levelname)s] %(message)s"

    def format(self, record: logging.LogRecord) -> str:
        if record.levelno == logging.INFO:
            self._style._fmt = self._info_fmt
        else:
            self._style._fmt = self._other_fmt
        return super().format(record)


def _init_logger() -> None:
    """Configure reasonable defaults for the maturin.import_hook logger."""
    logger.setLevel(logging.INFO)
    handler = logging.StreamHandler()
    handler.setLevel(logging.INFO)
    formatter = _LevelDependentFormatter()
    handler.setFormatter(formatter)
    logger.addHandler(handler)
    logger.propagate = False


_init_logger()


def reset_logger() -> None:
    """Clear the custom configuration on the maturin import hook logger
    and have it propagate messages to the root logger instead.
    """
    logger.propagate = True
    logger.setLevel(logging.NOTSET)
    for handler in logger.handlers:
        logger.removeHandler(handler)
