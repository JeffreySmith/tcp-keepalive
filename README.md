# Usage

You will need rust installed on your machine. See [here](https://rust-lang.org/tools/install/) if you do not already have it installed.

Run `cargo run -- server` in more than one terminal.

Connect a client (or multiple clients) to it by running `cargo run -- client -t 127.0.0.1:3000` in a terminal (or multiple), then hit ctrl-c (or send SIGTERM) in the same window as the active server.

Any connections should automatically jump to one of currently running servers.

Steps to test:

1. In a terminal window, run `cargo run -- server`
2. In another terminal window, run `cargo run -- server` (repeat this step as many times as desired)
3. In another terminal window, run `cargo run -- client -t 127.0.0.1:3000` (repeat this step as many times as desired)
4. In the terminal window of the active server, hit ctrl-c to stop the server. It should hand off all of its connection to one of the other running servers, with no interruption to the client(s).

Known issue:

~~There is ocassionally a bug where a socket doesn't get cleaned up properly. If you experience unexpected disconnections, try deleting all of the sockets related to this application and then run the new servers.
ie for the default socket directory, `rm /tmp/socket-forward*`~~

The above issue should be fixed.
