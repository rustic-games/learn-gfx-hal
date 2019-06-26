# Generalizing HalState

* **Guest Author:** [AlterionX](https://github.com/AlterionX)

## Initial Attempt

So the first thing to do when generalizing code to multiple types is to identify
what we are trying to generalize. For us, that's the backend. If you've been
following the tutorial, it's been all the classes under `back::`. Now that we
know what to abstract, we can go ahead and start adding in generics/type parameters.

We should take a good look at our `HalState` struct first, so here it is:

```rust
pub struct HalState {
  current_frame: usize,
  frames_in_flight: usize,
  in_flight_fences: Vec<<back::Backend as Backend>::Fence>,
  render_finished_semaphores: Vec<<back::Backend as Backend>::Semaphore>,
  image_available_semaphores: Vec<<back::Backend as Backend>::Semaphore>,
  command_buffers: Vec<CommandBuffer<back::Backend, Graphics, MultiShot, Primary>>,
  command_pool: ManuallyDrop<CommandPool<back::Backend, Graphics>>,
  framebuffers: Vec<<back::Backend as Backend>::Framebuffer>,
  image_views: Vec<(<back::Backend as Backend>::ImageView)>,
  render_pass: ManuallyDrop<<back::Backend as Backend>::RenderPass>,
  render_area: Rect,
  queue_group: ManuallyDrop<QueueGroup<back::Backend, Graphics>>,
  swapchain: ManuallyDrop<<back::Backend as Backend>::Swapchain>,
  device: ManuallyDrop<back::Device>,
  _adapter: Adapter<back::Backend>,
  _surface: <back::Backend as Backend>::Surface,
  _instance: ManuallyDrop<back::Instance>,
}
```

Almost everything has a `back::` in it! And the most common, by far, is
`back::Backend`, so our next step is to abstract it. If we look through the type
declaration in `gfx-hal`'s backend crates, we'll see that this type implements a
trait called `Backend` and is used as such in our code. So then, we can force our
type parameter to have this trait by adding a type parameter with a trait bound.
This means the type parameter must implement the trait `Backend`, which seems to
be the requirement for our use case. So then, the type signature of HalState is:

```rust
struct HalState<B: Backend> { /* the fields, which will be generalized to B */ }
```

Well, you still need to declare the concrete type (a version of this type with
all the types concrete instead of abstract), so we will now need to declare our
struct:

```rust
struct OtherPlace {
  hal_state: HalState<back::Backend>
}
```

Notice that we can't get away from `back::Backend` completely -- and there's no
reason to. We can't stay in the abstract forever. However, this does let us
implement the `HalState` without worrying about the underlying system. This
tends to be good, so now let's move on and take a peek how to generalize the
fields.

We've added the type parameter, but we haven't abstracted the types yet! So now
let's go ahead and do that. The type we've added in is meant to replace
`back::Backend`.

If we look at the struct definition, the first in the list with `back::Backend`
is `in_flight_fences`:

```rust
  in_flight_fences: Vec<<B as Backend>::Fence>,
```

Simple enough, right? Well, now we need to change every time we wrote
`back::Backend`. The more astute might notice that this results in `B as
Backend` in many places, but we'll leave that alone for now. As it turns out,
it will be useful when we restructure things a little more later.

Ok, so we've replaced all the instances of `back::Backend`. But we still have
several cases where we're relying on the `back::` part! Here's a snippet showing
those cases:

```rust
pub struct HalState<B: Backend> {
  // Lotsa fields here
  device: ManuallyDrop<back::Device>,
  // A few more fields
  _instance: ManuallyDrop<back::Instance>,
}
```

We have two other fields! So how are we going to generalize these as well?
Remember that we're going to get rid of anything that has `back::` in it!

Well, we could replace these as well, the same way we did earlier. If we
look at `back::Device` and `back::Instance`, we can see that they have
the traits `Device` and `Instance`. So then, let's try that (relatively
blindly, as it will turn out):

```rust
pub struct HalState<B: Backend, D: Device, I: Instance> {
  /* Lotsa fields here */
  device: ManuallyDrop<D>,
  /* A few more fields */
  _instance: ManuallyDrop<I>,
}
```

Well, now the compiler's complaining that `Device` is missing a type parameter.
Well, if we look at Device's trait's declaration, it looks like:

```rust
pub trait Device<B: Backend>: Any + Send + Sync { /* things are here */ }
```

Looks like device is missing a `Backend` type parameter! Let's tack that on to
the `Device` trait bound for type parameter `D`. Now it looks like:

```rust
pub struct HalState<B: Backend, D: Device<B>, I: Instance> { /* fields! */ }
```

Alright, so everything might be working now!

And we can use it like this:

```rust
struct OtherPlace {
  hal_state: HalState<back::Backend, back::Device, back::Instance>
}
```

Hm, that's a bit of a mouthful, isn't it?

Well, at least we have all the types in the struct sorted out now. Let's move on
to the impl section:

```rust
impl HalState {
  fn new() {
    /* lotsa things get initialized and constructed */
  }
}
```

This doesn't compile at all, so let's listen to the compiler and add the needed
types:

```rust
impl <B: Backend, D: Device, I: Instance> HalState<B, D, I> { /* lotsa fns */ }
```

But now it complains about out new function! 

```rust
  fn new(w: &Window) {
    let inst = I::create("HalState", 1);
    /* lots more initializations */
  }
```

Well, turns out that create isn't part of the `Instance` trait, so we can't
construct it directly. Well, how do we get one then? Turns out, we can just
pass it in:

```rust
  fn new(w: &Window, inst: I) { /* lots more initializations */ }
```

Notice that we move I and not take a reference. This is because we want HalState
to manage the instance. But if you try to `cargo check` again, it will tell you
that `create_surface` isn't part of the `Instance` trait either. So we have to
do the same thing and pass it in. But if we do that, it won't be mutable and we
need to use `Surface` as mutable, so we need to declare this input to be mutable
as well:

```rust
  fn new(w: &Window, inst: I, mut surf: <B>::Surface) { /* lots more initializations */ }
```

Once again, notice how we moved surface instead of taking a reference. So then,
we can now call new like this:

```rust
fn some_other_thing() {
  /* lotsa things */
  let w = createWindow();
  let instance = back::Instance::create("HalState", 1);
  let surface = instance.create_surface(&w);
  let hal_state = HalState::new(&w, inst, surf);
  /* lotsa more things */
}
```

Okay. It's gotta work NOW right? No, not at all. If you try to compile the code,
the compiler ends up throwing up somewhere in the mess and spews out a list of
errors a mile long. Somewhere in there, there will be errors that look something
like this:

```
error[E0308]: try expression alternatives have incompatible types
   --> main.rs:some_line_number:some_character_number
    |
ln# |                 in_flight_fences.push(device.create_fence(true).map_err(|_| "Could not create a fence!")?);
    |                                       ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ expected associated type, found struct `renderer::back::native::Fence`
    |
    = note: expected type `<B as Backend>::Fence`
               found type `back::native::Fence`
/* lots more errors */
error[E0308]: mismatched types
   -->  main.rs:some_line_number:some_character_number
    |
ln# |             device: ManuallyDrop::new(device),
    |                                       ^^^^^^ expected type parameter, found associated type
    |
    = note: expected type `D`
               found type `<<I as Instance>::Backend as Backend>::Device`
```

So there's a mismatch of types where `device.create_fence` is returning
`renderer::back::native::Fence`. But if that's the case, why was it working
before we tried to generalize `Device`? Don't they have to be the same thing
then? And why is `D: Device<B>` not the same as longer `WeirdLongThing::Device`?

## A Brief Discussion of Traits and Associated Types

So we need to take a bit of a sidetrack to figure this one out.

One of the core concepts of Rust is traits. There are small, nicely packaged
interfaces. Or, at least, they appear to be. Within the most basic traits, there
is a list of function definitions that any struct (or blanket impls) need to
implement. However, you can have what are called "associative type"s attached to
these traits. Here's some sample `gfx-hal` code that showcases associated types:

```rust
pub trait Instance: Any + Send + Sync {
    type Backend: Backend;
    fn enumerate_adapters(&self) -> Vec<Adapter<Self::Backend>>;
}
```

If you look at the second line, you'll notice the `type` keyword, which is
usually used for type aliases. (If you don't know what type aliases are, I
recommend that you go read up/use Rust a bit more before coming back to this.)
Here, we can see that this type has no definition, and so can't be a type alias.
Well, not yet. This is an associated type. The impl of the trait is
responsible for defining this type alias as a type following the listed type
bound `Backend`.

## Continuing the Generalization

Okay, so let's think a litte bit about the `device` variable in the `new`
function. This comes from a long list of calls that start from `I`.

Turns out that `I` has a specific implementation that looks something like
`back::native::Instance` (well, for the Vulkan backend at least). So when we
create a device from this, we end up with that long weird type. What's important
to note is that the backend in `I` is not necessarily the same as the `B` we are
using as a `Backend`! That's because of the associated type `Backend` in `I`.
While this may have been confusing naming, it's now apparent what we need to do:
we need to tell the compiler that associated type `Backend` from `I` is the same
`Backend` that we are using.

We can do this by using some interesting syntax:

```rust
struct HalState<B: Backend, D: Device<B>, I: Instance<Backend = B> {
  /* Fields go here */
}
```

Okay, so now this is just getting unwieldy. It turns out that there is one more
ambiguity, so we need to actually use:

```rust
struct HalState<B: Backend<Device = D>, D: Device<B>, I: Instance<Backend = B> {
  /* Fields go here */
}
```

And then, we should have something that works. But that type is a bit ridiculous.
Let's see if we can fix that as well.

## Making a Better Generalization

Now if you look carefully, you'll realize that all of this is actually defined by
`I`! The type `B` is simply the associated type `I::Backend` and `D` is the
associated type `I::Backend::Device`!

Anyways, we can now replace the really, really long type with this:

```rust
struct HalState<I: Instance> { /* Fields go here */ }
```

Short and sweet, and so much cleaner.

Well, that's great, but now it doesn't compile anymore! So we need to patch up
two other sections of our code: the sections that use `B` or `D` now that we've
removed those type parameters and the parts we declare these type parameters.

The `B` (since we've left the ` as Backend` parts from earlier) can be replaced
with `I::Backend` and the `D`s from earlier can be replaced with `<I::Backend
as Backend>::Device`. The impl of `HalState` becomes

```rust
impl <I: Instance> HalState<I> {
  /* all the functions */
}
```

## Further enhancements

As it turns out, I really dislike writing `I::Backend` as opposed to `B`. While
I'm unaware of how to do this for the struct block, we will be able to, in some
distant future as of writing this, use the `type` keyword to alias `I::Backend`
to `B`. Alternatively, you can use the first method, but only on the impl blocks.
The impl would then only match for `Instance<Backend=B>`, but this is a nonissue
so far, from what I've seen.

As a final touch, we can actually implement functions for `HalState`s with
specific type parameters. Instead of an on-site use of:

```rust
fn some_other_thing() {
  /* lotsa things */
  let w = createWindow();
  let instance = back::Instance::create("HalState", 1);
  let surface = instance.create_surface(&w);
  let hal_state = HalState::new(&w, inst, surf);
  /* lotsa more things */
}
```

we can:

```rust
impl HalState<back::Instance> {
    fn create(&w: Window) {
        let instance = back::Instance::create("HalState", 1);
        let surface = instance.create_surface(&w);
        HalState::new(&w, inst, surf)
    }
}
type HalStateWithOurBackend = HalState<back::Instance>;

fn some_other_thing() {
  /* lotsa things */
  let w = createWindow();
  let hal_state = HalStateWithOutBackend::create(&w);
  /* lotsa more things */
}
```

If we further implement `HalState` for each backend, it is possible that any
users of `HalState` would also have access to `Instance` implementation
specific methods if they require them (or if they will even exist).

