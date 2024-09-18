# Prior Arts & Inspirations

## [Deno](https://github.com/denoland/deno)

As the project description suggests, Den and Deno is just one word apart, Deno has the most inspiration onto this
project out of all. Not only the inspirational use of tokio and swc comes from Deno, but that this project would also
attempt to follow the design philosophy of Deno as well. One key importance is that both Den and Deno denounce CommonJS
and prefer to use ES Modules -- giving you some gucci stuff such as HTTP loading because ESM import has to be static,
and that means the initial module graph is deterministic enough so we can cache most things around. The exception would
be [dynamic import](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Operators/import) though.

(TODO: finish up the elaboration)

## [txiki.js](https://github.com/saghul/txiki.js)

Another big inspiration for Den is from Txiki. In
fact, [I used to contribute a tiny part of it](https://github.com/saghul/txiki.js/pulls?q=is%3Apr+author%3Astevefan1999-personal+is%3Aclosed)
as early as from late 2019 util early 2020. Despite this I was still very unhappy with it, in particular I was not
content that it does not support Windows well, or the lack thereof, and I don't like the use of cURL either because it
is a huge dependency and security bug hotspot, plus I have to use CMake to build Txiki.

To be honest, CMake itself is already quite a pain in the ass to work with from the beginning, especially when you
consider that C/C++ is not a particularly good language to make modular software, you often get a lot of include header
error and template error message spamming, they both share a common point of being cryptic in nature, which at that
moment I started to give up on C/C++ altogether after self-learning it for the past 6 years.

It was also around this time (in 2020) I started to discover Rust out of curious for a school project of one of my
course to learn more about programming language (more specifically, COMP3021 Programming Language Paradigms in PolyU
Computing, unfortunately this course is now out-of-syllabus. Although I was expecting to learn things like Automata
theory using the famed [Dragon Book](https://en.wikipedia.org/wiki/Compilers:_Principles,_Techniques,_and_Tools), at
least I do get a chance to write my first IRL parser for a small language
using [SableCC](https://sablecc.org/) despite I accidentally made my first one in 2017 for my solution
to [Tiny Three-Pass Compiler in Codewars](https://www.codewars.com/kata/5265b0885fda8eac5900093b). RIP COMP3021)

Since learning Rust I always have the idea of reinventing another txiki.js but better, but I have put that on hold for
quite some time, you know, life goes on. It was not until I started contributing to
the [SurrealDB](https://github.com/surrealdb/surrealdb/pulls/stevefan1999-personal) project that I see the use of
rquickjs would I have reignited the idea of rewriting another JS runtime like txiki, but I have also started to get in
touch with Deno ever since I also started learning Rust.

Den is the result of the 5 days Easter holiday and I think it is cool enough to give it a longshot. Even if it does not
achieve (or contradicts) the goals I listed, at least I can put all my Rust knowledge to test.

Den can be seen as the combination of txiki and Deno together by having the following traits in no particular order (
with [T] being txiki, [D] being deno, and [R] being Rust, symbols that are prefixed with [-] means anti-thesis/doesn't
have/don't want to have/having rooms for improvement):

- **[-D,T]** Easily portable JS interpreter rather than having a bloated JIT compiler
- **[D,R,-T]** A safe programming that is noisy but at least not a footgun
    - **[R,-T]** And easy to integrate, by using derive and procedural macros to make meta-programs rather than
      text-replacement macro or templates that
      are [Turing-complete](https://en.wikipedia.org/wiki/Template_metaprogramming)
- **[R,-T]** A software building pipeline done right, at least easy to program, reproduce and cross-compile
    - I'd be glad to add deterministic and immutable build using Nix, but that's on TODO for now
- **[-D,R,-T]** An easy way to integrate into other program using library-based modular design
- **[-D,R,T]** Easily hackable to extend the base software -> "_Den picks others_", not "_other picks Den_"
    - In some sense, it is the _inverse_ of the rule exactly one level above
- **[D,R,T]** High performance multi-platform async support
- **[D,R,-T]** A sane standard library that is well-designed and fits modern design
    - _cough cough_ just look at how POSIX hates async AIO and prefer to block everything
- **[D,R]** Improvising and promoting the use of static typing. Static types FTW!
    - Type exists for a reason, that they are supposed to be a contract/guarantee that certain entities/actor should
      behave in this way
    - And you can collect the types together, prove their correctness a layer by layer like a logical assertion/theorem,
      so the type system can also be seen as an automated theorem prover that verifies type!
    - The connection between type system, Coq, Prolog and SMT solvers is not a coincidence!
    - (See [Curry–Howard correspondence](https://en.wikipedia.org/wiki/Curry%E2%80%93Howard_correspondence))
    - But people are too lazy to mark the right types. Terrible stuff like duck-typing should be avoided at all cost.
    - So we can still keep the proofing part using static type and strip them away on execution. Although this
      information are valuable for optimization and the runtime can't check whether the input is correct or not, but at
      least we human know, it is good enough!
    - So I have no idea why people hates static types and prefer to use Python though...such a nightmare for refactors

## [Dune](https://github.com/aalykiot/dune)

Dune is another hobbyist project that shares some software stack in Den, but like Deno it uses V8 instead of QuickJS.
As a result the performance should be much better than Den, but I would also view it as another "toy Deno" that I took
some code inspiration from. I just found it on a reddit post and I think it is an honorable mention.

## Footnotes: Design Philosophies

### Goals

Den will try to achieve these **as much as possible** (to a point it is _sufficient but not necessary_), listed in no
specific orders:

1. Minimalism
    - Every bit of stuff should be necessary
    - And if not, comment it out or mark it as a feature
2. Composable
    - It should work well with existing tools
    - Performance isn't an issue given we have extremely fast processor nowadays that even a mobile processor can beat a
      beefy PC in certain tasks
3. Safety's and correctness first
    - Don't use unsafe unless necessary
        - Don't fight the borrow checker too much, it exists for a good reason
    - **Use the right kind of algorithm and data structure, since this equals programs**
        - An example nutshell:
            - **If you saw something can be implemented as a trie, then use a trie**
        - Another example nutshell:
            - If you need to sort some stuff repeatedly, like frequently updating a stack based on its content
                - Rather than keep calling sort when you need it, you should instead use a priority queue
                - **But don't use a binary heap directly** despite priority queues are usually implemented with a binary
                  heap underneath
                    - This way we can swap the implementations easily for **inversion of control**
        - Even if it costs performance, if the benefits are far better (for example, it helps better concurrency
          control), then it is justifiable, so feel free to put in mutexes and read-write barriers.
    - If you saw some constructs that can be abstracted/generalized, then abstract/generalize it
4. While simple and effectiveness second [^1]
    - Things should be easy and intuitive to understand
        - Just do it right while keeping it short and precise
5. Follow the paradigm of [Literate Programming](https://wiki.c2.com/?LiterateProgramming)
    - In an essence, **the code should express itself without comments**
        - Name variables as good as you can
            - Because Den isn't leetcode/codeforces/hackerrank
            - So please refrain from using 'n', 'i', 'j', 'k', 'l' as variable names, since they are cryptic in
              nature
            - Other good examples, despite I often make the same mistakes too
                - "dp", "tbl", "window", "lcs", "ptr"
6. Prefer async and concurrency programming paradigm whenever possible
    - So if there is an async API, then use it, even if it takes more code. It exists for a reason.
    - It is best to add mechanisms to gracefully shutdown a future
        - Just like how a finite state machine should have a recovery/rollback/compensation action when it unexpectedly
          enters an illegal state
    - Warns the user they are using sync code when there is an async companion function for early replacement of sync
      code. _Preferably_ just an oneshot notice in the standard error maybe
7. Portable
    - Should build on many platforms out of the box, given that platform has tier 1/tier 2 support in Rust
    - Make sure there are no **hard** architecture/platform dependent stuff in the main crates
        - If it does, then split it into a feature or a separate crate (as Den is separated in workspaces)
        - Or even a separate project if you will
    - Test all the stuffs using real-life environments as much as possible
        - Well...because QEMU never attempted to be an accurate representation of the target environment...
        - So if you built something for ARM64, try testing it in Raspberry Pi and M1 Macs
            - This could unearth subtle and obscure bug based on architectural design differences, such as memory
              consistency model and processor execution order (in-order, out-of-order, speculative execution)

[^1]: https://uxmyths.com/post/115783813605/myth-34-simple-minimal

### Non-Goals

(TODO: separate the explanations to a different document)

Den does not attempt to do the following stuff, and gave explanations/contrapositions listed in no specific orders:

1. CommonJS and NodeJS Modules resolution
    - That is a cancer. So no, never ask me to do that. _If you want it, then you'll have to fork it (fork Den). But you
      already
      knew that_.
2. Small binary size
    - I don't care about it that much
        - It's like I'm not writing botnets anymore...who cares
    - Probably go up to a huge size with a huge standard library. Windows x64 build
    - But I gave you the ability to build a small binary for yourself out of the box anyway
3. Performance and memory usage
    - Clone whenever you want, cope with other entities to synchronize the state eventually.
    - Prioritize the use of persistent/immutable data structure whenever possible
4. Extensive documentation
    - Especially no need for verbose comments that repeats the same logic again and again
        - These are redundant information in logical sense, and nobody likes redundancy
        - Only make **annotative comments** on things that have **bad readability**
5. Atomism (aka "Do one thing and do it well")
    - It is fine to be complex, but it should not be complicated as much as possible
    - Examples of what counts as **complex but not complicated**:
        - All functional programming languages (apparently)
        - Writing a lot of traits for different kinds of behaviors
    - Examples of what counts as **complicated but not complex**:
        - Layering stuff with generic types while you can use dynamic dispatch to write lesser code. Again,
          performance and allocation isn't an issue.
        - Read-copy-update (used extensively in Linux kernel): It's just multi-version concurrency control (MVCC) in
          disguise, or better known as _data snapshot_. It is not hard to implement but things get spicy to use them
          correctly. In the end they are all optimistic concurrency control under the hood, while RCU does it in the
          worst way possible.
        - HTTP: when you try to make an apparently interactive session (that is stateful in nature) using stateless
          paradigm and a simple text format. It kinda worked except you may have just saved a hidden state
          somewhere, maybe in the headers IDK. And then HTTP/2 added compressed headers and pipelining...you can
          clearly see it is wrong from the very beginning, and now it needs more monkey-patches.
6. `no_std`
    - Den needs an async runtime (`tokio` specifically) at the moment, and an async runtime usually implies the need for
      a standard environment
    - Nor would `rquickjs` supports `no_std` environment in the near term
    - I do plan to spin it off to another project which focus more on embedded environments such as MCUs and SBCs when
      performance, code size and _universal portability_ do matter
7. Rust version stability
    - I will try to make it as stable as possible, but right now it needs unstable Rust

As a matter of fact...this is heavily inspired by some parts of
the [Unix philosophy](https://wiki.c2.com/?UnixDesignPhilosophy), except you slash the "do one thing
well" part out, and put the "batteries-included" part in from Python. I obviously don't like the way Unix
pipe things (streaming text format is very limited, and you should pass objects/tagged unions instead), and I don't like
their way to have one-process-per-action kind of things in Unix, as it will get hard to trace on situation such as
running under preemptive SMP environment that admits the possibility for migrating the program to run on multiple cores.

Although I don't like one big monolithic codepasta either, but I'm more of liking to put that in if it is something
valuable and vital in the long term. In other word, I prefer "small-enough" while being open to extension.

Allow me to put a tin-foil hat first and do not nag me why Linux is not prioritized, I don't use Linux as a daily driver
myself.
Despite having to use NixOS in 2022 for a brief moment in my life and to be honest it is not that bad, but GNOME is
still riddled with bugs.

Don't let me recall upon the horrors that one Linux ext4 corruption/blkmq bug I experienced on 4.19 before, which wiped
my whole Manjaro setup back in 2019. Since then, I went back to Windows, never switched back, been very skeptical to
Linux desktop and that's why I use WSL2 instead, and it covers 80% of my Linux use cases.

I know people would hate me for saying this but Linux desktop simply sucks. I still use Linux for Proxmox and Kubernetes
but not my desktop where I have my bread and butter.