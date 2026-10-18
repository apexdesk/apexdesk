# Access and ApexDesk databases

## What ApexDesk opens

ApexDesk opens `.mdb` and `.accdb` files and imports:

- tables;
- relations;
- queries.

It does not import:

- forms;
- reports;
- macros;
- VBA modules.

When a file has any of these, the import says how many were left out.

## Using an ApexDesk database from Access

An ApexDesk database (`.apexdb`) is a SQLite file. Access can link to it, or
import from it, through an ODBC driver for SQLite.

**1. Install the driver.**

- Use the SQLite ODBC driver by Christian Werner (`sqliteodbc`).
- Use the 64-bit driver with 64-bit Office, and the 32-bit driver with
  32-bit Office. Access shows its bitness in File > Account > About Access.

**2. Add a data source.**

1. Open *ODBC Data Sources* (the 64-bit or 32-bit one, to match Office).
2. Click Add and pick *SQLite3 ODBC Driver*.
3. Give the source a name and choose the `.apexdb` file as its database.

**3. Link the tables in Access.**

1. Go to External Data > New Data Source > From Other Sources > ODBC
   Database.
2. Choose *Link to the data source* to keep working on the same file, or
   *Import* to copy the tables into the Access file.
3. On the Machine Data Source tab, pick the source from step 2.
4. Choose the tables.

## Notes

- Leave out `apex_objects` and `apex_fields`. They hold ApexDesk's own forms,
  reports, queries and field descriptions, and are not data.
- Access can change rows only in tables that have a primary key. Tables
  without one link read-only.
- Dates are stored as ISO text (`2026-10-17 09:30:00`). The driver reads them
  as dates in columns declared `DATETIME`, as ApexDesk declares them.
- Close the database in ApexDesk before changing it from Access. SQLite
  locks the whole file while it writes.
