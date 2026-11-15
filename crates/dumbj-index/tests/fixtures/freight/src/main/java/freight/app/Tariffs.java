package freight.app;

import freight.RateTables;
import freight.Tariff;

class Tariffs {
    boolean check(Tariff tariff) {
        return RateTables.standardRate() > 0 && RateTables.isCheap() && tariff.isActive();
    }
}
