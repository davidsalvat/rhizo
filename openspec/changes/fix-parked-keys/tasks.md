# Tasks

## 1. Keys

- [x] 1.1 `Net::set_secret` and keys from `siphash24(secret, counter)`, unique among parked sockets (D1); host tests `park_keys_use_siphash_2_4` (reference vector) and `a_parked_socket_answers_only_its_own_key` (64 counter guesses and neighbours of known keys take nothing); verify `cargo test -p rhizo-net`
- [x] 1.2 The kernel sets the secret at boot from its randomness; verify `cargo xtask check`
- [ ] 1.3 Parked connections still come back after suspend (in-kernel tests of `keep-tcp-across-suspend`); verify `cargo xtask test` on both architectures
