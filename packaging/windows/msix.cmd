@echo off
rem Builds dist\ApexDesk-<version>-x64.msix on Windows. UNTESTED (written on
rem Linux, where MSIX packages can't be built).
rem
rem Needs, once:
rem   Visual Studio's build tools (for cargo build)
rem   the Windows SDK, with makeappx and signtool on the PATH (they are in
rem   C:\Program Files (x86)\Windows Kits\10\bin\<version>\x64)
rem
rem The package holds ApexDesk.exe, msix\AppxManifest.xml (its Version set
rem from Cargo.toml) and msix\Assets. Its identity is the one Partner Center
rem gives; the Store signs what is uploaded there. For a copy installed
rem outside the Store, APEXDESK_SIGN_CERT (a .pfx whose subject is the
rem manifest's Publisher) and APEXDESK_SIGN_PASSWORD sign the package.
setlocal enabledelayedexpansion
set here=%~dp0
pushd "%here%..\.."
for /f "tokens=3 delims== " %%v in ('findstr /b "version" Cargo.toml') do if not defined version set version=%%~v
cargo build --release -p apex-app || exit /b 1
set stage=%CD%\target\msix-stage
if exist "%stage%" rmdir /s /q "%stage%"
mkdir "%stage%\Assets"
copy /y target\release\apexdesk.exe "%stage%\ApexDesk.exe" >nul
copy /y "%here%msix\Assets\*.png" "%stage%\Assets\" >nul
copy /y "%here%README.txt" "%stage%\" >nul
copy /y THIRD-PARTY-NOTICES.md "%stage%\" >nul
copy /y LICENSE "%stage%\" >nul
copy /y EULA.md "%stage%\" >nul
copy /y EULA.es.md "%stage%\" >nul
rem MSIX versions have four parts: 1.0.0 -> 1.0.0.0.
powershell -NoProfile -Command "(Get-Content -Raw '%here%msix\AppxManifest.xml') -creplace ' Version=\"[0-9.]+\"', ' Version=\"%version%.0\"' | Set-Content -NoNewline -Encoding UTF8 '%stage%\AppxManifest.xml'" || exit /b 1
if defined APEXDESK_SIGN_CERT (
  signtool sign /fd SHA256 /f "%APEXDESK_SIGN_CERT%" /p "%APEXDESK_SIGN_PASSWORD%" /tr http://timestamp.digicert.com /td SHA256 "%stage%\ApexDesk.exe" || exit /b 1
)
if not exist dist mkdir dist
set out=dist\ApexDesk-%version%-x64.msix
makeappx pack /o /d "%stage%" /p "%out%" || exit /b 1
if defined APEXDESK_SIGN_CERT (
  signtool sign /fd SHA256 /f "%APEXDESK_SIGN_CERT%" /p "%APEXDESK_SIGN_PASSWORD%" /tr http://timestamp.digicert.com /td SHA256 "%out%" || exit /b 1
)
echo %out%
popd
exit /b 0
