/**
 * A minimal off-request-path worker. The handler in server.ts pushes a task
 * and returns immediately; this drains tasks on a separate tick, after the
 * HTTP response has already gone out. That's the actual mechanism behind
 * "fast ack, work off the request path" — not just wrapping the same
 * synchronous work in a Promise, which would still run before Express
 * flushes the response on a fast-enough client.
 */
export class AsyncWorkQueue {
  private readonly tasks: Array<() => Promise<void> | void> = [];
  private draining = false;

  push(task: () => Promise<void> | void): void {
    this.tasks.push(task);
    this.scheduleDrain();
  }

  private scheduleDrain(): void {
    if (this.draining) return;
    this.draining = true;
    setImmediate(() => {
      void this.drain();
    });
  }

  private async drain(): Promise<void> {
    let task = this.tasks.shift();
    while (task) {
      try {
        await task();
      } catch (error) {
        console.error("[work-queue] task failed:", error);
      }
      task = this.tasks.shift();
    }
    this.draining = false;
  }
}
