import math

from .cffi_mixed import ffi


class Line:
    def __init__(self, x1: float, y1: float, x2: float, y2: float):
        # You can pass a tuple/list or a dict as value for a public rust struct
        self.start = ffi.new("Point *", {"x": x1, "y": y1})
        self.end = ffi.new("Point *", (x2, y2))

    def length(self) -> float:
        """Returns the length of the line."""
        return math.sqrt(
            (self.end.x - self.start.x) ** 2 + (self.end.y - self.start.y) ** 2
        )

    def __str__(self) -> str:
        return "Line from ({},{}) to ({},{})".format(
            self.start.x, self.start.y, self.end.x, self.end.y
        )
