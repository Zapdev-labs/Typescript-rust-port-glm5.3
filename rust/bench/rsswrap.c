/* rsswrap — run a command, print wall time + peak RSS (ru_maxrss) via wait4.
 *
 * The test rig (Omarchy) has no /usr/bin/time; this is the portable
 * replacement used by every timed benchmark run (rust/SPEC.md §15).
 *
 * Build: cc -O2 -o rsswrap rsswrap.c   (see rust/scripts/rig-setup.sh)
 * Usage:  rsswrap CMD [ARGS...]
/* Output (stderr, so the wrapped command's stdout stays clean):
 *   wall_s=<sec> maxrss_kb=<kb> exit=<code>
 */
#include <stdio.h>
#include <stdlib.h>
#include <sys/resource.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

int main(int argc, char **argv) {
    if (argc < 2) {
        fprintf(stderr, "usage: rsswrap CMD [ARGS...]\n");
        return 2;
    }
    struct timespec a, b;
    clock_gettime(CLOCK_MONOTONIC, &a);
    pid_t pid = fork();
    if (pid == 0) {
        execvp(argv[1], argv + 1);
        perror("execvp");
        _exit(127);
    }
    int status = 0;
    struct rusage ru;
    if (wait4(pid, &status, 0, &ru) == -1) {
        perror("wait4");
        return 1;
    }
    clock_gettime(CLOCK_MONOTONIC, &b);
    double wall = (double)(b.tv_sec - a.tv_sec) + (double)(b.tv_nsec - a.tv_nsec) / 1e9;
    int code = WIFEXITED(status) ? WEXITSTATUS(status) : 1;
    fprintf(stderr, "wall_s=%.3f maxrss_kb=%ld exit=%d\n", wall, ru.ru_maxrss, code);
    return code;
}
