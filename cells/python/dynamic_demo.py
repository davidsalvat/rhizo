# Run by the distribution's dynamically linked `python3` in a cell (change
# `add-dynamic-linking`): `json` loads its C accelerator, a shared object
# the interpreter opens with `dlopen` at run time.
import json

print(json.dumps({"dynamic": True, "sum": sum(range(10))}, sort_keys=True))
