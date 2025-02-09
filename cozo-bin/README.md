# Cozo (standalone executable)

[![server](https://img.shields.io/github/v/release/cozodb/cozo)](https://github.com/cozodb/cozo/releases)

This document describes how to set up cozo (standalone executable).
To learn how to use CozoDB (CozoScript), read the [docs](https://docs.cozodb.org/en/latest/index.html).

## Download

The standalone executable for Cozo can be downloaded from the [release page](https://github.com/cozodb/cozo/releases).
Look for those with names `cozo-*`.
Those with names `cozo_all-*` supports additional storage backends
such as [TiKV](https://tikv.org/) storage, but are larger.

## Starting the server

Run the cozo command in a terminal:

```bash
./cozo server
```

This starts an in-memory, non-persistent database.
For more options such as how to run a persistent database with other storage engines,
see `./cozo server -h`

To stop Cozo, press `CTRL-C`, or send `SIGTERM` to the process with e.g. `kill`.

## The REPL

Run `./cozo repl` to enter a terminal-based REPL. The engine options can be used when
invoking the executable to choose the backend.

You can use the following meta ops in the REPL:

* `%set <KEY> <VALUE>`: set a parameter that can be used in queries.
* `%unset <KEY>`: unset a parameter.
* `%clear`: unset all parameters.
* `%params`: print all set parameters.
* `%import <FILE OR URL>`: import data in JSON format from the file or URL. 
* `%save <FILE>`: the result of the next successful query will be saved in JSON format in a file instead of printed on screen. If `<FILE>` is omitted, then the effect of any previous `%save` command is nullified. 
* `%backup <FILE>`: the current database will be backed up into the file.
* `%restore <FILE>`: restore the data in the backup to the current database. The current database must be empty.

## The query API

Queries are run by sending HTTP POST requests to the server. 
By default, the API endpoint is `http://127.0.0.1:9070/text-query`. 
A JSON body of the following form is expected:
```json
{
    "script": "<COZOSCRIPT QUERY STRING>",
    "params": {}
}
```
params should be an object of named parameters. For example, if params is `{"num": 1}`, 
then `$num` can be used anywhere in your query string where an expression is expected. 
Always use params instead of concatenating strings when you need parametrized queries.

The HTTP API always responds in JSON. If a request is successful, then its `"ok"` field will be `true`,
and the `"rows"` field will contain the data for the resulting relation, and `"headers"` will contain
the headers. If an error occurs, then `"ok"` will contain `false`, the error message will be in `"message"`
and a nicely-formatted diagnostic will be in `"display"` if available.

> Cozo is designed to run in a trusted environment and be used by trusted clients. 
> It does not come with elaborate authentication and security features. 
> If you must access Cozo remotely, you are responsible for setting up firewalls, encryptions and proxies yourself.
> 
> As a guard against users accidentally exposing sensitive data, 
> If you bind Cozo to non-loopback addresses, 
> Cozo will generate a token string and require all queries 
> to provide the token string in the HTTP header field `x-cozo-auth`. 
> The warning printed when you start Cozo with a 
> non-default binding will tell you where to find the token string. 
> This “security measure” is not considered sufficient for any purpose 
> and is only intended as a last defence against carelessness.
> 
> In some environments, setting the header may be difficult or impossible
> for some of the APIs. In this case you can pass the token in the query parameter `auth`.

## API

* `POST /text-query`, described above.
* `GET /export/{relations: String}`, where `relations` is a comma-separated list of relations to export.
* `PUT /import`, import data into the database. Data should be in `application/json` MIME type in the body,
   in the same format as returned in the `data` field in the `/export` API.
* `POST /backup`, backup database, should supply a JSON body of the form `{"path": <PATH>}`
* `POST /import-from-backup`, import data into the database from a backup. Should supply a JSON body 
   of the form `{"path": <PATH>, "relations": <ARRAY OF RELATION NAMES>}`.
* `GET /`, if you open this in your browser and open your developer tools, you will be able to use
   a very simple client to query this database.

> For `import` and `import-from-backup`, triggers are _not_ run for the relations, if any exists.
If you need to activate triggers, use queries with parameters.

The following are experimental:

* `GET(SSE) /changes/{relation: String}` get changes when mutations are made against a relation, relies on [SSE](https://developer.mozilla.org/en-US/docs/Web/API/Server-sent_events/Using_server-sent_events).
* `GET(SSE) /rules/{name: String}` register a custom fixed rule and receive requests for computation.
  Query parameter `arity` must also be present.
* `POST /rule-result/{id}` post results of custom fixed rule computation back to the server, used together with the last API.
* `POST /transact` start a multi-statement transaction, the ID returned is used in the following two APIs.
  Need to set the `write=true` query parameter if mutations are present.
* `POST /transact/{id}` do queries inside a multi-statement transaction, JSON payload expected is the same as for `/text-query`. 
* `PUT /transact/{id}` commit or abort a multi-statement transaction. JSON payload is of the form `{"abort": <bool>}`, pass `false` for commit and `true` for abort. If you forget to do this, a resource leak results, even for read-only transactions.


## Building

Building `cozo` requires a [Rust toolchain](https://rustup.rs). Run

```bash
cargo build --release -p cozo-bin -F compact -F storage-rocksdb
```
