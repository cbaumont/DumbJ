package com.example.demo;

import java.util.List;
import java.util.*;
import static java.lang.Math.max;
import static java.util.Collections.*;

public class Structure<T> extends Base implements Runnable, java.io.Serializable {
    private static final int MAX = 1, MIN = 0;
    private List<String> names;
    public Structure(String name) { this.names = null; }
    @Override public void run() {
        int local = MAX;
        for (String s : names) { foo(s, local); }
        Runnable r = () -> { int inner = 2; };
        Outer.Inner x = new Outer.Inner();
        x.go(); this.names.size(); Math.abs(1); String.valueOf(local);
        try (var in = open()) { } catch (Exception e) { }
    }
    static <U> U foo(U value, int n) { return value; }
    interface Shape { double area(); }
    enum Color { RED, GREEN; void paint() {} }
    record Point(int x, int y) { }
    @interface Marker { }
    class Inner { void go() {} }
}
