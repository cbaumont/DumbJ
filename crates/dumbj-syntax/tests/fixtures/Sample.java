package com.example.demo;

import java.util.List;

/** A greeter. */
@Deprecated
public final class Greeter<T extends Comparable<T>> implements Runnable {
    private static final int MAX = 0x10;
    private final String name;

    public Greeter(String name) {
        this.name = name;
    }

    @Override
    public void run() {
        // line comment
        for (int i = 0; i < MAX; i++) {
            System.out.println("Hello, " + name + "\n");
        }
        List<String> items = List.of("a", "b");
        boolean ok = items != null && true;
        char c = 'x';
        double d = 1.5e3;
    }
}
