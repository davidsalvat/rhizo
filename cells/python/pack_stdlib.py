# The standard library a CPython cell needs, as a tree of compiled modules
# (design D2 of `add-cpython`). Run by the CPython that will run the scripts:
#
#     python pack_stdlib.py <out-dir> <script>...
#
# Each script runs once, without arguments, with its output discarded; each
# standard-library module that the scripts and the interpreter's start-up
# imported is compiled to a sourceless `.pyc` (never checked against a
# source) at its relative path under <out-dir>, the directory that becomes
# `lib/python3.X`.
import io
import sys

out, scripts = sys.argv[1], sys.argv[2:]
shown, sys.stdout = sys.stdout, io.StringIO()
for script in scripts:
    with open(script) as f:
        code = compile(f.read(), script, "exec")
    sys.argv = [script]
    exec(code, {"__name__": "__main__", "__file__": script})
sys.stdout = shown
modules = dict(sys.modules)

import os  # noqa: E402 (after the snapshot of what the scripts imported)
import py_compile  # noqa: E402

# CPython finds its standard library by `os` (the landmark), which a run
# with `-S` does not import.
modules.setdefault("os", os)

import shutil  # noqa: E402

lib = os.path.dirname(os.__file__)
for name in sorted(modules):
    path = getattr(modules[name], "__file__", None) or ""
    if not path.startswith(lib + os.sep):
        continue
    rel = os.path.relpath(path, lib)
    if rel.startswith("site-packages"):
        continue
    if path.endswith(".so"):
        # An extension module (a dynamically linked interpreter's
        # `lib-dynload`): copied as it is.
        os.makedirs(os.path.dirname(os.path.join(out, rel)), exist_ok=True)
        shutil.copyfile(path, os.path.join(out, rel))
        continue
    if not path.endswith(".py"):
        continue
    py_compile.compile(
        path,
        cfile=os.path.join(out, rel + "c"),
        dfile=rel,
        doraise=True,
        invalidation_mode=py_compile.PycInvalidationMode.UNCHECKED_HASH,
    )
