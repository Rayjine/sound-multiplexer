# Makefile for Sound Multiplexer

PACKAGE_NAME = sound-multiplexer
VERSION = 1.0.0
PYTHON = python3
PIP = pip3

.PHONY: help install uninstall clean build rpm deb test lint format

help:
	@echo "Sound Multiplexer Build System"
	@echo ""
	@echo "Available targets:"
	@echo "  install     - Install the application system-wide"
	@echo "  install-user- Install for current user only"
	@echo "  uninstall   - Uninstall the application"
	@echo "  clean       - Clean build artifacts"
	@echo "  build       - Build the package"
	@echo "  rpm         - Build RPM package"
	@echo "  deb         - Build DEB package"
	@echo "  test        - Run tests"
	@echo "  lint        - Run code linting"
	@echo "  format      - Format code"
	@echo ""

# Install system-wide (requires root)
install: build
	$(PYTHON) setup.py install --root=$(DESTDIR)
	@echo "Installation complete. You can now run 'sound-multiplexer' from anywhere."

# Install for current user only
install-user: build
	$(PIP) install --user .
	@echo "User installation complete. Make sure ~/.local/bin is in your PATH."

# Uninstall
uninstall:
	$(PIP) uninstall -y $(PACKAGE_NAME)

# Clean build artifacts
clean:
	rm -rf build/
	rm -rf dist/
	rm -rf *.egg-info/
	find . -type d -name __pycache__ -exec rm -rf {} +
	find . -type f -name "*.pyc" -delete
	find . -type f -name "*.pyo" -delete

# Build package
build: clean
	$(PYTHON) setup.py sdist bdist_wheel

# Build RPM package (Fedora/RHEL/openSUSE)
rpm: build
	mkdir -p packaging/rpmbuild/{BUILD,RPMS,SOURCES,SPECS,SRPMS}
	cp dist/$(PACKAGE_NAME)-$(VERSION).tar.gz packaging/rpmbuild/SOURCES/
	rpmbuild --define "_topdir $(PWD)/packaging/rpmbuild" \
		-ba packaging/$(PACKAGE_NAME).spec
	@echo "RPM packages built in packaging/rpmbuild/RPMS/"

# Build DEB package (Debian/Ubuntu)
deb: build
	mkdir -p packaging/debian
	cd packaging/debian && \
	tar -xzf ../../dist/$(PACKAGE_NAME)-$(VERSION).tar.gz && \
	cd $(PACKAGE_NAME)-$(VERSION) && \
	dh_make -f ../../../dist/$(PACKAGE_NAME)-$(VERSION).tar.gz -s -y && \
	dpkg-buildpackage -us -uc
	@echo "DEB package built in packaging/debian/"

# Run tests
test:
	$(PYTHON) -m pytest tests/ -v

# Run linting
lint:
	$(PYTHON) -m flake8 src/
	$(PYTHON) -m mypy src/

# Format code
format:
	$(PYTHON) -m black src/
	$(PYTHON) -m isort src/

# Quick local install for development
dev-install:
	$(PIP) install -e .

# Create distribution tarball
dist: clean
	$(PYTHON) setup.py sdist

# Install dependencies
deps:
	$(PIP) install -r requirements.txt

# Install development dependencies
dev-deps: deps
	$(PIP) install pytest black flake8 mypy isort wheel twine