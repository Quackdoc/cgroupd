name := 'cgroupd'
rootdir := ''
prefix := '/usr'
sysconfdir := '/etc'
cargo-target-dir := env('CARGO_TARGET_DIR', 'target')
profile := 'release'

base-dir := absolute_path(clean(rootdir / prefix))
conf-dir := absolute_path(clean(rootdir / sysconfdir))

bin-src := cargo-target-dir / profile
cgroupd-dst := base-dir / 'bin' / 'cgroupd'
cgctl-dst := base-dir / 'bin' / 'cgctl'
conf-dst := conf-dir / 'cgroupd.conf'
dbus-dst := base-dir / 'share/dbus-1/system.d/org.cgroupd.conf'
varlink-dst := base-dir / 'share/varlink/interfaces/org.cgroupd.Manager.varlink'
doc-dir := base-dir / 'share/doc' / name

# Compile with release profile by default
default: build-release

# Compile with debug profile
build-debug *args:
    cargo build {{ args }}

# Compile with release profile
build-release *args: (build-debug '--release' args)

# Compile with a vendored tarball (offline)
build-vendored *args: vendor-extract (build-release '--frozen --offline' args)

# Run clippy
check *args:
    cargo clippy --all-targets {{ args }} -- -W clippy::pedantic

# Remove Cargo build artifacts
clean:
    cargo clean

# Also remove .cargo and vendored dependencies
clean-dist: clean
    rm -rf .cargo vendor vendor.tar

# Vendor Cargo dependencies locally
vendor:
    mkdir -p .cargo
    cargo vendor --locked > .cargo/config.toml
    tar pcf vendor.tar .cargo vendor
    rm -rf .cargo vendor

# Extract vendored dependencies
vendor-extract:
    rm -rf vendor
    tar pxf vendor.tar

# Install binaries, config, D-Bus policy, varlink interface and PAM example
# (use profile=debug for debug builds). Does not touch /etc/pam.d.
install:
    install -Dm0755 '{{ bin-src / 'cgroupd' }}' '{{ cgroupd-dst }}'
    install -Dm0755 '{{ bin-src / 'cgctl' }}' '{{ cgctl-dst }}'
    install -Dm0644 etc/cgroupd.conf.example '{{ conf-dst }}'
    install -Dm0644 etc/dbus-1/system.d/org.cgroupd.conf '{{ dbus-dst }}'
    install -Dm0644 etc/org.cgroupd.Manager.varlink '{{ varlink-dst }}'
    install -Dm0644 etc/pam.d/session.example '{{ doc-dir / 'pam-session.example' }}'
    @if [ -z '{{ rootdir }}' ]; then \
        echo; \
        echo "==> Optional per-login cgroups are NOT configured automatically."; \
        echo "    Add to /etc/pam.d/system-login, sshd, etc. (after any pam_systemd/pam_elogind line):"; \
        echo "      session optional pam_exec.so quiet {{ cgctl-dst }} pam"; \
        echo "    See {{ doc-dir / 'pam-session.example' }}"; \
    fi

# Install the dinit service
install-dinit:
    install -Dm0644 etc/dinit/cgroupd '{{ conf-dir / 'dinit.d/cgroupd' }}'

# Install the OpenRC service
install-openrc:
    install -Dm0755 etc/openrc/cgroupd.initd '{{ conf-dir / 'init.d/cgroupd' }}'

# Install the runit service (set sv-dir=/etc/sv on Void)
sv-dir := conf-dir / 'runit/sv'
install-runit:
    install -Dm0755 etc/runit/cgroupd/run '{{ sv-dir / 'cgroupd/run' }}'
    install -Dm0755 etc/runit/cgroupd/log/run '{{ sv-dir / 'cgroupd/log/run' }}'

# Install the s6 service
install-s6:
    install -Dm0755 etc/s6/cgroupd/run '{{ conf-dir / 's6/sv/cgroupd/run' }}'

# Remove everything installed by `install` and the init recipes
uninstall:
    rm -f '{{ cgroupd-dst }}' '{{ cgctl-dst }}' '{{ conf-dst }}' '{{ dbus-dst }}' '{{ varlink-dst }}'
    rm -rf '{{ doc-dir }}'
    rm -f '{{ conf-dir / 'dinit.d/cgroupd' }}' '{{ conf-dir / 'init.d/cgroupd' }}'
    rm -rf '{{ sv-dir / 'cgroupd' }}' '{{ conf-dir / 's6/sv/cgroupd' }}'
