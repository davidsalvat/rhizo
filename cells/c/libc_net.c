// Descriptors that wait and sockets through a real libc (specs `linux-abi`
// and `networking`): a pipe and an event counter with poll, a TCP echo over
// 127.0.0.1 with epoll, select, UDP datagrams with sendmsg and recvmsg, and a
// Unix socket. Run in an empty directory, it prints the same on Linux and in
// a cell.
#define _GNU_SOURCE
#include <arpa/inet.h>
#include <ctype.h>
#include <errno.h>
#include <fcntl.h>
#include <netinet/in.h>
#include <netinet/tcp.h>
#include <poll.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/epoll.h>
#include <sys/eventfd.h>
#include <sys/select.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/uio.h>
#include <sys/un.h>
#include <unistd.h>

static int fail(const char *what) {
    printf("FAILED %s: errno %d\n", what, errno);
    return 1;
}

static struct sockaddr *sa(void *addr) { return (struct sockaddr *)addr; }

int main(void) {
    char buf[256];
    signal(SIGPIPE, SIG_IGN);

    // A pipe, polled.
    int p[2];
    if (pipe2(p, O_NONBLOCK)) return fail("pipe2");
    struct pollfd pf[2] = {{p[0], POLLIN, 0}, {p[1], POLLOUT, 0}};
    int n = poll(pf, 2, 0);
    printf("pipe poll: %d ready, read end %s, write end %s\n", n,
           pf[0].revents ? "ready" : "empty",
           pf[1].revents & POLLOUT ? "writable" : "full");
    errno = 0;
    ssize_t got = read(p[0], buf, 1);
    printf("empty read: %zd errno %d\n", got, errno);
    if (write(p[1], "through a pipe", 14) != 14) return fail("write pipe");
    got = read(p[0], buf, sizeof buf);
    printf("pipe: %.*s\n", (int)got, buf);
    close(p[1]);
    pf[0].revents = 0;
    poll(pf, 1, 10);
    got = read(p[0], buf, 1);
    printf("closed pipe: revents %#x, read %zd\n", pf[0].revents, got);
    close(p[0]);

    // An event counter.
    int e = eventfd(3, EFD_NONBLOCK);
    if (e < 0) return fail("eventfd");
    uint64_t v = 4;
    if (write(e, &v, 8) != 8 || read(e, &v, 8) != 8) return fail("eventfd io");
    printf("eventfd: %llu\n", (unsigned long long)v);
    errno = 0;
    got = read(e, &v, 8);
    printf("drained: %zd errno %d\n", got, errno);
    struct pollfd t = {e, POLLIN, 0};
    printf("poll timeout: %d\n", poll(&t, 1, 20));

    // A TCP echo over loopback, with epoll.
    int l = socket(AF_INET, SOCK_STREAM, 0);
    int one = 1;
    setsockopt(l, SOL_SOCKET, SO_REUSEADDR, &one, sizeof one);
    struct sockaddr_in a = {.sin_family = AF_INET,
                            .sin_addr.s_addr = htonl(INADDR_LOOPBACK)};
    if (bind(l, sa(&a), sizeof a) || listen(l, 4)) return fail("bind/listen");
    socklen_t len = sizeof a;
    getsockname(l, sa(&a), &len);
    printf("listening on 127.0.0.1: %s\n", ntohs(a.sin_port) ? "a port" : "no port");
    int c = socket(AF_INET, SOCK_STREAM | SOCK_NONBLOCK, 0);
    int r = connect(c, sa(&a), sizeof a);
    printf("connect: %s\n", r == 0 || errno == EINPROGRESS ? "started" : "failed");
    int ep = epoll_create1(EPOLL_CLOEXEC);
    struct epoll_event ev = {.events = EPOLLIN, .data.u64 = 42};
    epoll_ctl(ep, EPOLL_CTL_ADD, l, &ev);
    struct epoll_event ready[4];
    n = epoll_wait(ep, ready, 4, 1000);
    printf("epoll: %d ready, data %llu\n", n, (unsigned long long)ready[0].data.u64);
    int s = accept4(l, NULL, NULL, SOCK_NONBLOCK);
    if (s < 0) return fail("accept4");
    struct pollfd cw = {c, POLLOUT, 0};
    poll(&cw, 1, 1000);
    int err = -1;
    len = sizeof err;
    getsockopt(c, SOL_SOCKET, SO_ERROR, &err, &len);
    printf("connected: writable %d, SO_ERROR %d\n", !!(cw.revents & POLLOUT), err);
    setsockopt(c, IPPROTO_TCP, TCP_NODELAY, &one, sizeof one);
    ev.events = EPOLLIN | EPOLLRDHUP;
    ev.data.u64 = 7;
    epoll_ctl(ep, EPOLL_CTL_ADD, s, &ev);
    errno = 0;
    got = recv(s, buf, sizeof buf, 0);
    printf("nothing yet: %zd errno %d\n", got, errno);
    if (send(c, "hello, server", 13, 0) != 13) return fail("send");
    n = epoll_wait(ep, ready, 4, 1000);
    printf("epoll: %d ready, data %llu, events %#x\n", n,
           (unsigned long long)ready[0].data.u64, ready[0].events);
    got = recv(s, buf, sizeof buf, 0);
    for (ssize_t i = 0; i < got; i++) buf[i] = toupper((unsigned char)buf[i]);
    send(s, buf, got, 0);
    struct pollfd cr = {c, POLLIN, 0};
    poll(&cr, 1, 1000);
    got = recv(c, buf, sizeof buf, 0);
    printf("echo: %.*s\n", (int)got, buf);
    struct sockaddr_in peer, mine;
    len = sizeof peer;
    getpeername(s, sa(&peer), &len);
    len = sizeof mine;
    getsockname(c, sa(&mine), &len);
    printf("peer is 127.0.0.1: %d, ports match: %d\n",
           peer.sin_addr.s_addr == htonl(INADDR_LOOPBACK), peer.sin_port == mine.sin_port);
    shutdown(c, SHUT_WR);
    n = epoll_wait(ep, ready, 4, 1000);
    unsigned events = ready[0].events;
    got = recv(s, buf, sizeof buf, 0);
    printf("after shutdown: %d ready, events %#x, read %zd\n", n, events, got);
    close(c);
    close(s);
    close(l);
    int d = socket(AF_INET, SOCK_STREAM, 0);
    errno = 0;
    r = connect(d, sa(&a), sizeof a);
    printf("nobody listens: %d errno %d\n", r, errno);
    close(d);

    // select on a pipe.
    if (pipe(p)) return fail("pipe");
    write(p[1], "x", 1);
    fd_set rd;
    FD_ZERO(&rd);
    FD_SET(p[0], &rd);
    struct timeval tv = {1, 0};
    n = select(p[0] + 1, &rd, NULL, NULL, &tv);
    printf("select: %d, readable %d\n", n, FD_ISSET(p[0], &rd) != 0);

    // UDP datagrams.
    int u1 = socket(AF_INET, SOCK_DGRAM, 0), u2 = socket(AF_INET, SOCK_DGRAM, 0);
    struct sockaddr_in b = {.sin_family = AF_INET, .sin_addr.s_addr = htonl(INADDR_LOOPBACK)};
    if (bind(u2, sa(&b), sizeof b)) return fail("bind udp");
    len = sizeof b;
    getsockname(u2, sa(&b), &len);
    sendto(u1, "datagram one", 12, 0, sa(&b), sizeof b);
    struct iovec out[2] = {{"data", 4}, {"gram two", 8}};
    struct msghdr m = {.msg_name = &b, .msg_namelen = sizeof b, .msg_iov = out, .msg_iovlen = 2};
    sendmsg(u1, &m, 0);
    struct sockaddr_in from;
    len = sizeof from;
    got = recvfrom(u2, buf, 8, 0, sa(&from), &len);
    printf("udp: %.*s (cut), from 127.0.0.1: %d\n", (int)got, buf,
           from.sin_addr.s_addr == htonl(INADDR_LOOPBACK));
    char p1[5], p2[20];
    struct iovec in[2] = {{p1, 5}, {p2, 20}};
    struct msghdr mm = {.msg_iov = in, .msg_iovlen = 2};
    got = recvmsg(u2, &mm, 0);
    printf("udp recvmsg: %zd bytes, %.5s|%.*s, flags %d\n", got, p1, (int)got - 5, p2,
           mm.msg_flags);

    // A Unix socket, named by a file.
    int us = socket(AF_UNIX, SOCK_STREAM, 0);
    struct sockaddr_un ua = {.sun_family = AF_UNIX};
    strcpy(ua.sun_path, "echo.sock");
    if (bind(us, sa(&ua), sizeof ua) || listen(us, 1)) return fail("bind unix");
    struct stat st;
    if (stat("echo.sock", &st)) return fail("stat");
    printf("unix: socket file %d\n", S_ISSOCK(st.st_mode) != 0);
    int uc = socket(AF_UNIX, SOCK_STREAM, 0);
    if (connect(uc, sa(&ua), sizeof ua)) return fail("connect unix");
    int ud = accept(us, NULL, NULL);
    write(uc, "over unix", 9);
    got = read(ud, buf, sizeof buf);
    printf("unix: %.*s\n", (int)got, buf);
    int again = socket(AF_UNIX, SOCK_STREAM, 0);
    errno = 0;
    r = bind(again, sa(&ua), sizeof ua);
    printf("unix again: %d errno %d\n", r, errno);
    unlink("echo.sock");
    int sv[2];
    if (socketpair(AF_UNIX, SOCK_STREAM, 0, sv)) return fail("socketpair");
    write(sv[0], "pair", 4);
    got = read(sv[1], buf, sizeof buf);
    printf("socketpair: %.*s\n", (int)got, buf);
    printf("net ok\n");
    return 0;
}
