# The standard library a CPython cell needs, as a tree of compiled modules
# (design D2 of `add-cpython`), with the installed packages it uses (change
# `add-pip-packages`). Run by the CPython that will run the scripts:
#
#     python pack_stdlib.py <out-dir> <script>...
#
# Each script runs once, with its output discarded; a script argument may
# carry the script's own arguments after it, each on a line of its own.
# Each module of the standard library or of `site-packages` that the
# scripts and the interpreter's start-up imported is compiled to a
# sourceless `.pyc` (never checked against a source) at its relative path
# under <out-dir>, the directory that becomes `lib/python3.X`; extension
# modules are copied as they are. From `site-packages` also come the data
# files next to an imported package's modules, and whole the libraries a
# wheel vendors (`<name>.libs`, found by its extension modules' run path)
# and the metadata of what is installed (`<name>.dist-info`, licenses
# included).
import io
import sys

out, scripts = sys.argv[1], sys.argv[2:]
shown, sys.stdout = sys.stdout, io.StringIO()
for script in scripts:
    sys.argv = script.split("\n")
    with open(sys.argv[0]) as f:
        code = compile(f.read(), sys.argv[0], "exec")
    exec(code, {"__name__": "__main__", "__file__": sys.argv[0]})
sys.stdout = shown
modules = dict(sys.modules)

import os  # noqa: E402 (after the snapshot of what the scripts imported)
import py_compile  # noqa: E402

# CPython finds its standard library by `os` (the landmark), which a run
# with `-S` does not import.
modules.setdefault("os", os)

import shutil  # noqa: E402

lib = os.path.dirname(os.__file__)
site = os.path.join(lib, "site-packages")


def copy(rel):
    os.makedirs(os.path.dirname(os.path.join(out, rel)), exist_ok=True)
    shutil.copyfile(os.path.join(lib, rel), os.path.join(out, rel))


for name in sorted(modules):
    path = getattr(modules[name], "__file__", None) or ""
    if not path.startswith(lib + os.sep):
        continue
    rel = os.path.relpath(path, lib)
    if path.endswith(".so"):
        # An extension module (a dynamically linked interpreter's
        # `lib-dynload`, or an installed package's): copied as it is.
        copy(rel)
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
    if rel.startswith("site-packages" + os.sep) and path.endswith("__init__.py"):
        package = os.path.dirname(path)
        for entry in sorted(os.listdir(package)):
            data = os.path.join(package, entry)
            if os.path.isfile(data) and not entry.endswith((".py", ".pyi", ".pxd", ".so")):
                copy(os.path.relpath(data, lib))
if os.path.isdir(site):
    for entry in sorted(os.listdir(site)):
        if entry.endswith((".libs", ".dist-info")):
            shutil.copytree(os.path.join(site, entry), os.path.join(out, "site-packages", entry))
