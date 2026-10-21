The fake's own record of what it was asked and what it did.

A live test asserts "sent exactly once" against this, never against the
database of the system under test. The database records what vsms *believes*
it did; the ledger records what the provider *received*. A bug that made
dispatch resubmit while believing it had not would leave the database
looking right and the ledger showing two submits.
