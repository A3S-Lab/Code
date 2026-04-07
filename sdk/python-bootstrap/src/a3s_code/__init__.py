from ._bootstrap import load_native_module

_native = load_native_module()

__doc__ = getattr(_native, "__doc__", None)
__all__ = [name for name in dir(_native) if not name.startswith("_")]

for _name in __all__:
    globals()[_name] = getattr(_native, _name)


def __getattr__(name: str):
    return getattr(_native, name)


def __dir__():
    return sorted(set(globals()) | set(dir(_native)))
