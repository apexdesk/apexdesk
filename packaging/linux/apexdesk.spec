# RPM spec for ApexDesk, driven by packaging/linux/rpm.sh (which passes the
# version and the folder with the built program).
Name:           apexdesk
Version:        %{apex_version}
Release:        1
Summary:        Lightweight spreadsheet and word processor
License:        LicenseRef-Proprietary
URL:            https://github.com/apexdesk/apexdesk
Requires:       curl
Recommends:     (gstreamer1-plugins-base or gstreamer-plugins-base)
Recommends:     (gstreamer1-plugins-good or gstreamer-plugins-good)
Recommends:     xdg-utils
Suggests:       hunspell-en
Obsoletes:      apexdesk-pdfium < %{version}-%{release}
Provides:       apexdesk-pdfium = %{version}-%{release}
%global debug_package %{nil}
%global __strip /bin/true

%description
ApexDesk opens and saves xlsx, docx, ods, odt, csv and Markdown files,
with formulas, charts, mail merge, digital signatures and PDF export.

%install
install -Dm755 %{apex_src}/apexdesk %{buildroot}/usr/lib/apexdesk/apexdesk
mkdir -p %{buildroot}/usr/bin
ln -s ../lib/apexdesk/apexdesk %{buildroot}/usr/bin/apexdesk
install -Dm644 %{apex_src}/apexdesk.desktop %{buildroot}/usr/share/applications/apexdesk.desktop
install -Dm644 %{apex_src}/apexdesk-mime.xml %{buildroot}/usr/share/mime/packages/apexdesk.xml
install -Dm644 %{apex_src}/apexdesk.svg %{buildroot}/usr/share/icons/hicolor/scalable/apps/apexdesk.svg
install -Dm644 %{apex_src}/apexdesk-256.png %{buildroot}/usr/share/icons/hicolor/256x256/apps/apexdesk.png
install -Dm644 %{apex_src}/policy.example %{buildroot}/usr/share/doc/apexdesk/policy.example
for f in LICENSE EULA.md EULA.es.md THIRD-PARTY-NOTICES.md; do
    install -Dm644 %{apex_src}/$f %{buildroot}/usr/share/doc/apexdesk/$f
done

%files
/usr/lib/apexdesk/apexdesk
/usr/bin/apexdesk
/usr/share/applications/apexdesk.desktop
/usr/share/mime/packages/apexdesk.xml
/usr/share/icons/hicolor/scalable/apps/apexdesk.svg
/usr/share/icons/hicolor/256x256/apps/apexdesk.png
%doc /usr/share/doc/apexdesk/policy.example
%license /usr/share/doc/apexdesk/LICENSE
%license /usr/share/doc/apexdesk/EULA.md
%license /usr/share/doc/apexdesk/EULA.es.md
%license /usr/share/doc/apexdesk/THIRD-PARTY-NOTICES.md
