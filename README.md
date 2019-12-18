# A simplified implementation of Nodes event loop and async handling

This is the example code which belongs to the gitbook [The Node Experiment - Exploring Async Basics With Rust](https://cfsamson.github.io/book-exploring-async-basics/), a gitbook explaining how concurrency is handled by operating systems, programming languages and runtimes.

Node is a good example since they use two different strategies for handling async and parallel execution depending on if the task is mostly IO bound or CPU bound. Since it's well known and there is already many articles trying to explain how it works, I think it's cool to take a deeper dive and try to understand this more in detail while we investigate how async programming is working.

This is part of my goal to explain Futures in Rust from the ground up. To understand even the most high level concept truly we need to understand what's happening on the low level or else there will be too much magic which prevents any profound understanding of the subject. 

**Updates:**

### 2019-12-18

Working on the book explaining the Epoll/IOCP/Kqueue implementation backing this event loop I have made some minor revisions to the API, and a bigger change to the underlying `IOCP` implementation.

- Interests is now expressed using constants: `Interests::readable()` is now `Interests::READABLE`
- `Token` is refactored to a plain `usize` instead of wrapping it, simplifying the code. That means it's no longer need to call `Token::id()::value()` to get the value of the token.
- Some combinations of "javascript" gave a fault when trying to exit the loop on Windows. This turned out to be caused by an oversight on my part on how `CompletionKey` worked. It's now fixed.
- Pinned the dependency of `minimio` to the `node-experiment` branch