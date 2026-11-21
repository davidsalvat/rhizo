# numpy in a cell (change `add-pip-packages`): the dynamically linked
# CPython loads numpy's C extensions and the OpenBLAS they need with
# `dlopen`, from a manylinux wheel. With the argument `wait` it prints
# `ready` once numpy is imported and sleeps 50 ms, so the host can suspend,
# fork or freeze it there (a template that imported numpy once). Then it
# computes: integer array operations, a linear solve and a determinant
# (LAPACK), a ufunc and a matrix product (BLAS). Deterministic: the same
# output on Linux and in a cell, whatever kernels OpenBLAS picks for the CPU
# (every result is exact or rounded).
import sys
import time

import numpy as np

if sys.argv[1:] == ["wait"]:
    print("ready", flush=True)
    time.sleep(0.05)
a = np.arange(1, 13, dtype=np.int64).reshape(3, 4)
print("numpy", np.__version__)
print("sum", int(a.sum()), "columns", a.sum(axis=0).tolist(), "trace", int((a.T @ a).trace()))
m = np.array([[4.0, 1.0, 2.0], [1.0, 5.0, 3.0], [2.0, 3.0, 6.0]])
x = np.linalg.solve(m, m @ np.array([1.0, -2.0, 3.0]))
print("solve", [round(float(v), 6) for v in x], "det", round(float(np.linalg.det(m)), 6))
print("hypot", np.hypot([3.0, 5.0, 8.0], [4.0, 12.0, 15.0]).tolist())
print("matmul", float((np.ones((64, 64)) @ np.full((64, 64), 0.5)).sum()))
