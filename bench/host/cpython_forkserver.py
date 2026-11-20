# The Linux side of `workload.cpython_warm_us` (design D4 of `add-cpython`),
# run by `linux_cpython.c` in the benchmark VM next to `agent_demo.py`:
#
#     python3 /app/cpython_forkserver.py <runs>
#
# A CPython process that imported and warmed up `agent_demo` forks one child
# per request, the pre-fork pattern of servers. Each child answers, as a cell
# forked from the template does, and exits through the interpreter's normal
# exit. Prints `warm_us=<median>`: from the fork to the child's exit reaped.
import os
import sys
import time

import agent_demo

runs = int(sys.argv[1])
request = agent_demo.warm()
answers, into = os.pipe()
samples = []
for _ in range(runs):
    t0 = time.perf_counter_ns()
    pid = os.fork()
    if pid == 0:
        os.dup2(into, 1)
        agent_demo.answer(request)
        print("cpython ok")
        sys.exit(0)
    _, status = os.waitpid(pid, 0)
    samples.append(time.perf_counter_ns() - t0)
    if status != 0 or not os.read(answers, 1 << 16).endswith(b"cpython ok\n"):
        sys.exit("a forked child failed")
samples.sort()
print("warm_us=%.1f" % (samples[len(samples) // 2] / 1000))
