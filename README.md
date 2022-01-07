# seaport

Note: This library is under heavy development and almost nothing is implemented yet. Stay tuned.

### Feature status

| feature                | status |
| ---------------------- | ------ |
| sockets + reliability  | 50%    |
| congestion control     | nyi    |
| virtual connections    | nyi    |
| fragmentation          | nyi    |
| multiplexing           | nyi    |

- reliability:
  - lacking client-side implementation, but most of the server-side code has been refactored into something
    side-agnostic (`peer` module). This allows it to be re-used for the client-side implementation.
  - untested.