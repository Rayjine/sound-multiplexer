Name:           sound-multiplexer
Version:        1.0.0
Release:        1%{?dist}
Summary:        GUI application for multiplexing audio output to multiple devices

License:        GPL-3.0-or-later
URL:            https://github.com/rayjine/sound-multiplexer
Source0:        %{name}-%{version}.tar.gz

BuildArch:      noarch
BuildRequires:  python3-devel
BuildRequires:  python3-setuptools
BuildRequires:  python3-pip
BuildRequires:  desktop-file-utils

Requires:       python3
Requires:       python3-PyQt6
Requires:       python3-pulsectl
Requires:       pulseaudio
Requires:       pulseaudio-utils

%description
Sound Multiplexer is a GUI application that allows you to play audio
simultaneously on multiple output devices using PulseAudio. Features include:
- Real-time device detection and management
- Individual volume controls per device
- Dark and light theme support
- System audio synchronization
- Intelligent device type detection with icons

%prep
%autosetup -n %{name}-%{version}

%build
%py3_build

%install
%py3_install

# Install desktop file
desktop-file-install \
    --dir=%{buildroot}%{_datadir}/applications \
    packaging/sound-multiplexer.desktop

# Install icon
mkdir -p %{buildroot}%{_datadir}/pixmaps
install -m 644 packaging/sound-multiplexer.png %{buildroot}%{_datadir}/pixmaps/

# Install documentation
mkdir -p %{buildroot}%{_docdir}/%{name}
install -m 644 README.md %{buildroot}%{_docdir}/%{name}/
install -m 644 docs/USER_GUIDE.md %{buildroot}%{_docdir}/%{name}/
install -m 644 docs/TECHNICAL.md %{buildroot}%{_docdir}/%{name}/

%files
%license LICENSE
%doc %{_docdir}/%{name}/
%{python3_sitelib}/src/
%{python3_sitelib}/sound_multiplexer-%{version}-py%{python3_version}.egg-info/
%{_bindir}/sound-multiplexer
%{_bindir}/sound-multiplexer-gui
%{_datadir}/applications/sound-multiplexer.desktop
%{_datadir}/pixmaps/sound-multiplexer.png

%post
/bin/touch --no-create %{_datadir}/icons/hicolor &>/dev/null || :
update-desktop-database &> /dev/null || :

%postun
if [ $1 -eq 0 ] ; then
    /bin/touch --no-create %{_datadir}/icons/hicolor &>/dev/null
    /usr/bin/gtk-update-icon-cache %{_datadir}/icons/hicolor &>/dev/null || :
fi
update-desktop-database &> /dev/null || :

%posttrans
/usr/bin/gtk-update-icon-cache %{_datadir}/icons/hicolor &>/dev/null || :

%changelog
* %(date "+%a %b %d %Y") %{getenv:USER} <%{getenv:USER}@localhost> - 1.0.0-1
- Initial RPM package
- GUI application for audio multiplexing
- Support for multiple audio output devices
- Real-time volume control and device management
- Dark/light theme support