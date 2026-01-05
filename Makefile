PREFIX ?= /usr/local
BINDIR = $(PREFIX)/bin
MANDIR = $(PREFIX)/share/man/man1

all: build

build:
	cargo build --release

install: build
	install -Dm755 target/release/truetm $(DESTDIR)$(BINDIR)/truetm
	install -Dm644 truetm.1 $(DESTDIR)$(MANDIR)/truetm.1

uninstall:
	rm -f $(DESTDIR)$(BINDIR)/truetm
	rm -f $(DESTDIR)$(MANDIR)/truetm.1

clean:
	cargo clean

.PHONY: all build install uninstall clean
