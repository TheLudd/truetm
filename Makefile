PREFIX ?= /usr/local
BINDIR = $(PREFIX)/bin
MANDIR = $(PREFIX)/share/man/man1

all: build

build:
	cargo build --release

install: build
	install -Dm755 target/release/simplex $(DESTDIR)$(BINDIR)/simplex
	install -Dm644 simplex.1 $(DESTDIR)$(MANDIR)/simplex.1

uninstall:
	rm -f $(DESTDIR)$(BINDIR)/simplex
	rm -f $(DESTDIR)$(MANDIR)/simplex.1

clean:
	cargo clean

.PHONY: all build install uninstall clean
