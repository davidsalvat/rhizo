## ADDED Requirements

### Requirement: Frames from the network are hostile input
The kernel SHALL treat every frame the card delivers as written by anyone
on the network. No sequence of frames SHALL panic the kernel, open or close
a socket, change another cell's sockets or connections (except one whose
peer's addresses the frames carry, which TCP and ARP do not authenticate),
send a cell's bytes to any host but its connection's peer, or keep kernel
memory once the sockets it reached are closed.

#### Scenario: Hostile frames next to another cell's connection
- **WHEN** a host on the network sends valid, changed, cut, replayed or random frames (TCP to listening, closed and connected ports, UDP, IPv4 fragments and options, ICMP, ARP, DHCP replies) while a cell's connection exchanges data with another host
- **THEN** the kernel does not panic, the number of open sockets does not change, that connection receives exactly the bytes its peer sent, its bytes leave only towards its peer, and once every socket is closed the kernel heap holds what it held before

#### Scenario: Hostile DHCP server
- **WHEN** a card asking for a lease receives changed, cut or random DHCP replies
- **THEN** the kernel does not panic, and the card's address, if it has one, is a unicast address

### Requirement: A flood cannot hold the kernel
One poll of the card SHALL take at most one receive ring of frames (64)
and then ask to be polled again at once, so that frames arriving faster
than the kernel handles them cannot hold the CPU that polls the card.

#### Scenario: Flood
- **WHEN** frames keep arriving faster than the kernel handles them
- **THEN** a poll of the card returns after 64 frames and asks to be polled again at once
