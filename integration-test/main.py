#!/usr/bin/env python3

import get_fourtytwo

if __name__ == '__main__':
    assert get_fourtytwo.DummyClass.get_42() == 42
    print(get_fourtytwo.DummyClass.get_42())
