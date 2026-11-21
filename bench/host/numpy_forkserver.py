# The Linux side of `workload.numpy_warm_us` (design D6 of
# `add-pip-packages`), run by `linux_numpy.c` in the benchmark VM next to
# `numpy_demo.py`:
#
#     python3 /app/numpy_forkserver.py <runs>
#
# A CPython process that imported numpy forks one child per request, the
# pre-fork pattern of servers. Each child runs the script's computation, as
# a cell forked from the template frozen after `import numpy` does, and
# exits through the interpreter's normal exit. Prints `warm_us=<median>`:
# from the fork to the child's exit reaped.
import os
import sys
import time

import numpy  # noqa: F401 (imported once, before the forks)

runs = int(sys.argv[1])
script = os.path.join(os.path.dirname(__file__), "numpy_demo.py")
with open(script) as f:
    code = compile(f.read(), script, "exec")
answers, into = os.pipe()
samples = []
for _ in range(runs):
    t0 = time.perf_counter_ns()
    pid = os.fork()
    if pid == 0:
        os.dup2(into, 1)
        sys.argv = [script]
        exec(code, {"__name__": "__main__"})
        sys.exit(0)
    _, status = os.waitpid(pid, 0)
    samples.append(time.perf_counter_ns() - t0)
    if status != 0 or b"matmul " not in os.read(answers, 1 << 16):
        sys.exit("a forked child failed")
samples.sort()
print("warm_us=%.1f" % (samples[len(samples) // 2] / 1000))
