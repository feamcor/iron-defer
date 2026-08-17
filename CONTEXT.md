# iron-defer

The domain of durable background task execution: tasks are enqueued into queues, claimed and executed by workers under time-boxed leases, and returned to the queue by a sweeper when leases expire.

## Language

### Task lifecycle

**Task**:
A unit of durable work, identified by an id, carrying a typed payload through a lifecycle of states.
_Avoid_: Job, message, work item

**Task Kind**:
The stable discriminator that selects the handler responsible for a task's payload.
_Avoid_: Type, handler name, topic

**Payload**:
The data a task carries, serialized for storage and handed to its handler at execution.
_Avoid_: Body, args, message

**Task State**:
A task is always in exactly one of six states: **Pending**, **Running**, **Completed**, **Failed**, **Cancelled**, or **Suspended**.
_Avoid_: Queued, active, processing, succeeded, done, canceled

**Enqueue**:
The act of placing a task into a queue.
_Avoid_: Submit, publish, schedule

**Claim**:
The atomic act of a worker taking ownership of a pending task.
_Avoid_: Dequeue, pick up, reserve

**Lease**:
The time-boxed claim a worker holds on a task; when it expires, the task becomes a zombie.
_Avoid_: Lock, reservation

**Attempt**:
One claim of a task by a worker, from the claim to its end — success, failure, cancellation, or lease expiry.
_Avoid_: Run, try, execution

**Execute**:
The handler invocation for one attempt of a task.
_Avoid_: Run, process, dispatch

**Retry**:
An attempt of a task after an earlier attempt ended in failure or expiry.
_Avoid_: Re-run, re-execution

**Backoff**:
The jittered delay a task waits before its next attempt.
_Avoid_: Cooldown, wait period

**Zombie Task**:
A task whose lease expired while it was still claimed, because its worker stopped before finishing.
_Avoid_: Orphaned task, stuck task, stale task

**Sweep**:
The act of returning zombie tasks to the queue for another attempt.
_Avoid_: Reap, recover, resurrect

**In-flight Task**:
A task currently claimed by a worker.
_Avoid_: Active task, live task

**Drain**:
The shutdown phase in which in-flight tasks finish and remaining claims are released.
_Avoid_: Flush, wind down

**Cancellation**:
The act of asking an executing task to stop at its next cooperation point.
_Avoid_: Kill, abort, terminate

**Suspend**:
The act of an executing task pausing itself mid-work with a checkpoint, awaiting human action.
_Avoid_: Pause, freeze, defer

**Checkpoint**:
Progress data an executing task saves mid-attempt, used by a later attempt to resume rather than restart.
_Avoid_: Snapshot, state dump, savepoint

**Signal**:
A payload delivered to an executing task to change its behavior without cancelling it.
_Avoid_: Command, poke, control message

**Idempotency Key**:
A caller-supplied key under which a duplicate enqueue of the same task is a no-op for a limited time.
_Avoid_: Dedupe key, request id, correlation key

**Region**:
A geographic label restricting a task to workers in that region.
_Avoid_: Zone, datacenter, location

**Audit Log**:
The append-only record of a task's state transitions and who caused them.
_Avoid_: History, trail, journal

### System parts

**Queue**:
A named channel of tasks with its own concurrency bound and ordering.
_Avoid_: Topic, table, channel

**Worker**:
The part of the system that claims and executes tasks.
_Avoid_: Consumer, processor, runner

**Sweeper**:
The part of the system that finds zombie tasks and sweeps them back into the queue.
_Avoid_: Reaper, janitor, recovery loop

**Engine**:
A running instance of the system, embedded in a host application or running standalone.
_Avoid_: Broker, client, instance

### Delivery semantics

**At-least-once**:
The guarantee that an accepted task is executed one or more times, never zero.
_Avoid_: Best effort, reliable

**Retry Policy**:
The bound on attempts and the backoff between them for a task or queue.
_Avoid_: Retry strategy, retry plan
