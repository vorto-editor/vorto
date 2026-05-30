# Sample Makefile to exercise syntax highlighting, indents,
# and folds in vorto. Open with `vorto assets/samples/hello.mk`.

CC      := cc
CFLAGS  := -Wall -Wextra -O2
SRCS    := $(wildcard *.c)
OBJS    := $(SRCS:.c=.o)
TARGET  := hello

.PHONY: all clean run

all: $(TARGET)

$(TARGET): $(OBJS)
	$(CC) $(CFLAGS) -o $@ $^

%.o: %.c
	$(CC) $(CFLAGS) -c $< -o $@

run: $(TARGET)
	./$(TARGET)

clean:
	rm -f $(OBJS) $(TARGET)

ifeq ($(DEBUG),1)
CFLAGS += -g -DDEBUG
endif
