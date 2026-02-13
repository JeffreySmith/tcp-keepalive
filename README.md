# Usage
You will need rust installed on your machine.


Run `cargo run -- server` in more than one terminal.

Connect a client (or multiple clients) to it by running `cargo run -- client -t 127.0.0.1:3000` in a terminal (or multiple), then hit ctrl-c in the same window as the active server.

Any connections should automatically jump to one of currently running servers.


Known issue:

There is ocassionally a bug where a socket doesn't get cleaned up properly. If you experience unexpected disconnections, try deleting all of the sockets related to this application and then run the new servers.
ie for the default socket directory, `rm /tmp/socket-forward*`
