# A simplified implementation of Nodes event loop and async handling

This is indended to be part of a gitbook explaining how asynchronisity is handled by operating systems, programming languages and runtimes.

Node is a good example since they use two different strategies for handeling async and parallell execution depending on if the task is mostly IO bound or CPU bound. It's also one of the most well known event loops, with a lot of oversimplified explanations around it and we have a nice opportunity to use this to help us understand this more in detail while we investigate how async programming is working.

This is part of my goal to explain Futures in Rust from the ground up, but to undertand even the most high level concept truly we need to understand what's happening on the low level or else there will be too much magic which prevents any profound understanding of the subject. 

Node is a fun and interesting example to use.