@echo off
rem Builds dist\ApexDesk-<version>-x64.msi on Windows. UNTESTED (written on
rem Linux, where Windows Installer packages can't be built).
rem
rem Needs, once:
rem   dotnet tool install --global wix --version 5.*
rem   wix extension add --global WixToolset.UI.wixext/5.0.2
rem   Visual Studio's build tools (for cargo build) and, to sign, signtool
rem
rem Optional component, when this folder is set:
rem   APEXDESK_DICT_DIR    .aff/.dic files (e.g. from LibreOffice's dictionaries)
rem APEXDESK_SIGN_CERT (a .pfx) and APEXDESK_SIGN_PASSWORD sign the program
rem and the package, with a timestamp.
setlocal enabledelayedexpansion
set here=%~dp0
pushd "%here%..\.."
for /f "tokens=2 delims== " %%v in ('findstr /b "version" Cargo.toml') do if not defined version set version=%%~v
cargo build --release -p apex-app || exit /b 1
set stage=%CD%\target\msi-stage
if exist "%stage%" rmdir /s /q "%stage%"
mkdir "%stage%"
copy /y target\release\apexdesk.exe "%stage%\ApexDesk.exe" >nul
copy /y "%here%README.txt" "%stage%\" >nul
copy /y THIRD-PARTY-NOTICES.md "%stage%\" >nul
if defined APEXDESK_SIGN_CERT (
  signtool sign /fd SHA256 /f "%APEXDESK_SIGN_CERT%" /p "%APEXDESK_SIGN_PASSWORD%" /tr http://timestamp.digicert.com /td SHA256 "%stage%\ApexDesk.exe" || exit /b 1
)
set di=0
if defined APEXDESK_DICT_DIR set di=1
if not exist dist mkdir dist
set out=dist\ApexDesk-%version%-x64.msi
pushd "%here%"
wix build apexdesk.wxs -arch x64 -ext WixToolset.UI.wixext ^
  -d Version=%version% -d Stage="%stage%" ^
  -d WithDictionaries=%di% -d DictSource="%APEXDESK_DICT_DIR%" ^
  -o "%here%..\..\%out%" || exit /b 1
popd
if defined APEXDESK_SIGN_CERT (
  signtool sign /fd SHA256 /f "%APEXDESK_SIGN_CERT%" /p "%APEXDESK_SIGN_PASSWORD%" /tr http://timestamp.digicert.com /td SHA256 "%out%" || exit /b 1
)
echo %out%
popd
exit /b 0
