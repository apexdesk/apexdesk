ApexDesk for Windows
====================

Run ApexDesk.exe: nothing to install. Settings are kept in
%APPDATA%\apexdesk. In the portable zip (a file named "portable" next to
ApexDesk.exe) they are kept in the "data" folder next to it instead, and
nothing is written to the profile or the registry.

Spelling: ApexDesk uses Hunspell dictionaries (.aff + .dic). It finds
those of LibreOffice if it is installed; otherwise put them in a
"dictionaries" folder next to ApexDesk.exe or in
%APPDATA%\apexdesk\dictionaries.

Printing goes through the PDF viewer set up for printing on the system
(for example Adobe Acrobat Reader or SumatraPDF). "Export to PDF" works
without one.

Policies: administrators can lock settings (macro security, templates
folder, default formats, proxy...) with Group Policy. Copy
policy\ApexDesk.admx and the language folders under policy\ to
C:\Windows\PolicyDefinitions (or the central store); the values go to
HKLM or HKCU\Software\Policies\ApexDesk. "apexdesk policy --list" shows
every policy and "apexdesk policy" the ones in force. Locked settings
appear greyed out in Options with "Set by your organization".
