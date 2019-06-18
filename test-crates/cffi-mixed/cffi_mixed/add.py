from .cffi_mixed import lib


def add_points(point1, point2):
    return lib.make_point(point1.x + point2.x, point1.y + point2.y)
