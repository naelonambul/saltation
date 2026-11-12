# Oracle Database image

Saltation uses Oracle Database 19c Enterprise Edition, built locally from Oracle's official container build scripts (`oracle/docker-images`, pinned in `build.sh`). The image is never published and the installation zip is never committed.

## Prerequisites

- The Oracle Database 19c installation zip for the target architecture, downloaded by the owner after accepting Oracle's license:
  - linux/arm64: `LINUX.ARM64_1919000_db_home.zip` (MD5 `6c39043ad12e11bcdc505184631e11a2`)
- Docker with at least 10 GB of memory for the build host or VM.

## Build

```sh
ORACLE_ZIP=/path/to/LINUX.ARM64_1919000_db_home.zip infra/oracle/build.sh
```

`ORACLE_ZIP` defaults to the owner's iCloud Drive `Downloads` folder on the Mac. The result is `saltation/oracle-ee:19.19`.

For linux/amd64 (the VMware lab), build on an amd64 host with `LINUX.X64_193000_db_home.zip`; the same scripts apply (spec N-10).

## Database settings

`setup/` runs once, when a new data volume is initialised:

- three redo log groups of 1 GB; NOARCHIVELOG (the image default);
- undo capped at 4 GB; `pga_aggregate_limit` 1.5 GB;
- in `ORCLPDB1`: tablespace `SALTATION_DATA`, tablespace `OUTBOX_TS` capped at 4 GB (spec F-36a), and the schema owner `SALTATION`, whose password comes from `SALTATION_DB_PASSWORD`.

## Smoke test

```sh
infra/oracle/smoke.sh
```

Creates a throwaway database and prints the version, log mode, redo, PGA limit, undo and outbox caps, then logs in as the schema owner.

## Moving the image to the OCI host

```sh
docker save saltation/oracle-ee:19.19 | ssh oracle docker load
```
