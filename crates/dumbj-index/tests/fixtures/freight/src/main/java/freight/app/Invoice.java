package freight.app;

import java.util.List;

record Invoice(int total, String customer) {
    static int sum(List<Invoice> all) {
        return all.stream().mapToInt(each -> each.total()).sum();
    }
}
