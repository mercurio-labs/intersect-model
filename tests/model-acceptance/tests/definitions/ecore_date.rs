use intersect_model::definitions::scalars::date::DateValue;
use serde_json::json;

#[test]
fn date_wire_preserves_full_millisecond_domain_without_float_or_timezone() {
    for milliseconds in [
        i64::MIN,
        i64::MIN + 1,
        -9007199254740993,
        -12219292800000,
        -1,
        0,
        1,
        9007199254740993,
        i64::MAX - 1,
        i64::MAX,
    ] {
        let date = DateValue::new(milliseconds);
        let wire = serde_json::to_value(date).unwrap();
        assert_eq!(
            wire,
            json!({"kind":"date","milliseconds":milliseconds.to_string()})
        );
        let restored: DateValue = serde_json::from_value(wire).unwrap();
        assert_eq!(restored.milliseconds(), milliseconds);
        assert_eq!(restored, date);
    }
    let mut state = 0x123456789abcdef0u64;
    for _ in 0..4096 {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        let date = DateValue::new(i64::from_ne_bytes(state.to_ne_bytes()));
        let restored: DateValue =
            serde_json::from_str(&serde_json::to_string(&date).unwrap()).unwrap();
        assert_eq!(restored, date);
    }
    assert!(DateValue::new(-1) < DateValue::new(0));
    assert!(DateValue::new(9) < DateValue::new(10));
}

#[test]
fn date_wire_rejects_lossy_noncanonical_and_foreign_values() {
    for milliseconds in [
        "",
        "+1",
        "01",
        "-0",
        " 1",
        "1 ",
        "1.0",
        "1e3",
        "١",
        "9223372036854775808",
        "-9223372036854775809",
    ] {
        assert!(
            serde_json::from_value::<DateValue>(json!({"kind":"date","milliseconds":milliseconds}))
                .is_err()
        );
    }
    for wire in [
        json!(null),
        json!(0),
        json!("0"),
        json!({"kind":"date","milliseconds":0}),
        json!({"kind":"date","milliseconds":"0","timezone":"UTC"}),
        json!({"kind":"int64","value":"0"}),
        json!({"kind":"date"}),
    ] {
        assert!(serde_json::from_value::<DateValue>(wire).is_err());
    }
}

#[test]
fn explicit_calendar_conversion_handles_epoch_cutover_rollover_and_offsets() {
    use intersect_model::definitions::scalars::date::{Calendar, DateFields};
    let fields = |year, month, day| DateFields {
        year,
        month,
        day,
        hour: 0,
        minute: 0,
        second: 0,
        millisecond: 0,
    };
    let g = Calendar::ProlepticGregorian;
    let h = Calendar::JulianGregorian1582;
    assert_eq!(
        DateValue::from_fields(fields(1970, 1, 1), g, 0)
            .unwrap()
            .milliseconds(),
        0
    );
    assert_eq!(
        DateValue::from_fields(fields(2000, 1, 1), g, 0)
            .unwrap()
            .milliseconds(),
        946684800000
    );
    assert_eq!(
        DateValue::from_fields(fields(1582, 10, 15), h, 0)
            .unwrap()
            .milliseconds(),
        -12219292800000
    );
    let before = DateValue::new(-12219292800000 - 86400000);
    assert_eq!(before.fields(h, 0).unwrap(), fields(1582, 10, 4));
    assert_eq!(before.fields(g, 0).unwrap(), fields(1582, 10, 14));
    assert_eq!(
        DateValue::from_fields(fields(1582, 10, 5), h, 0)
            .unwrap()
            .fields(h, 0)
            .unwrap(),
        fields(1582, 10, 15)
    );
    assert_eq!(
        DateValue::from_fields(fields(1900, 2, 29), g, 0)
            .unwrap()
            .fields(g, 0)
            .unwrap(),
        fields(1900, 3, 1)
    );
    assert_eq!(
        DateValue::from_fields(fields(2000, 2, 29), g, 0)
            .unwrap()
            .fields(g, 0)
            .unwrap(),
        fields(2000, 2, 29)
    );
    assert_eq!(
        DateValue::from_fields(fields(1970, 0, 32), g, 0)
            .unwrap()
            .milliseconds(),
        0
    );
    assert_eq!(
        DateValue::from_fields(fields(1970, 1, 1), g, 330)
            .unwrap()
            .milliseconds(),
        -19800000
    );
    let last = DateValue::new(-1).fields(g, 0).unwrap();
    assert_eq!(
        (
            last.year,
            last.month,
            last.day,
            last.hour,
            last.minute,
            last.second,
            last.millisecond
        ),
        (1969, 12, 31, 23, 59, 59, 999)
    );
    for year in [i64::MIN, i64::MAX] {
        assert!(DateValue::from_fields(fields(year, 1, 1), g, 0).is_err());
    }
    for offset in [-1440, 1440, i32::MIN, i32::MAX] {
        assert!(DateValue::new(0).fields(g, offset).is_err());
        assert!(DateValue::from_fields(fields(1970, 1, 1), g, offset).is_err());
    }
}

#[test]
fn both_calendars_round_trip_full_range_instants_with_explicit_offsets() {
    use intersect_model::definitions::scalars::date::Calendar;
    let mut instants = vec![i64::MIN, i64::MAX, -1, 0, 1, -12219292800000];
    let mut state = 19u64;
    for _ in 0..4096 {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        instants.push(i64::from_ne_bytes(state.to_ne_bytes()));
    }
    for calendar in [Calendar::ProlepticGregorian, Calendar::JulianGregorian1582] {
        for offset in [-1439, -300, 0, 330, 1439] {
            for milliseconds in &instants {
                let value = DateValue::new(*milliseconds);
                let fields = value.fields(calendar, offset).unwrap();
                assert_eq!(
                    DateValue::from_fields(fields, calendar, offset).unwrap(),
                    value
                );
            }
        }
    }
}

#[test]
fn calendar_matches_independent_python_datetime_vectors() {
    use intersect_model::definitions::scalars::date::{Calendar, DateFields};
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../fixtures/ecore-scalars/gregorian-dates.json"
    ))
    .unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 168);
    for case in cases {
        let fields = DateFields {
            year: case["year"].as_i64().unwrap(),
            month: case["month"].as_i64().unwrap(),
            day: case["day"].as_i64().unwrap(),
            hour: 12,
            minute: 34,
            second: 56,
            millisecond: 789,
        };
        let offset = i32::try_from(case["offset_minutes"].as_i64().unwrap()).unwrap();
        let expected: i64 = case["milliseconds"].as_str().unwrap().parse().unwrap();
        let actual = DateValue::from_fields(fields, Calendar::ProlepticGregorian, offset).unwrap();
        assert_eq!(actual.milliseconds(), expected, "{case}");
        assert_eq!(
            DateValue::new(expected)
                .fields(Calendar::ProlepticGregorian, offset)
                .unwrap(),
            fields
        );
    }
}

#[test]
fn five_date_formats_and_prefix_policy_are_explicit() {
    use intersect_model::definitions::scalars::date::{Calendar, DatePolicy, NoNamedZones};
    let policy = DatePolicy {
        calendar: Calendar::JulianGregorian1582,
        default_offset_minutes: 0,
        require_full_input: true,
    };
    for (text, expected, index) in [
        ("1970-01-01T05:30:00.000+0530", 0, 0),
        ("1970-01-01T05:30:00.000GMT+5:30", 0, 0),
        ("1970-01-01T00:00:00.000GMT", 0, 0),
        ("1970-01-01T00:00:00.000GMT-03:00", 10800000, 0),
        ("1970-01-01T00:00:00.1", 1, 1),
        ("1970-01-01T00:00:01", 1000, 2),
        ("1970-01-01T00:01", 60000, 3),
        ("1970-01-01", 0, 4),
        ("1970-01-01T00:00:00.1234", 1234, 1),
        ("1970-01-01T24:00", 86400000, 3),
        ("１９７０-０１-０１", 0, 4),
    ] {
        let result = policy.parse(text, &NoNamedZones).unwrap();
        assert_eq!(
            (
                result.value.milliseconds(),
                result.format_index,
                result.consumed
            ),
            (expected, index, text.len())
        );
    }
    assert_eq!(
        policy.format(DateValue::new(-1)).unwrap(),
        "1969-12-31T23:59:59.999+0000"
    );
    for text in [
        "",
        "1970",
        "1970-01",
        "1970-01-01junk",
        "1970-01-01T00:00:00.000+2460",
    ] {
        assert!(policy.parse(text, &NoNamedZones).is_err(), "{text}");
    }
    let prefix = DatePolicy {
        require_full_input: false,
        ..policy
    };
    let parsed = prefix.parse("1970-01-01junk", &NoNamedZones).unwrap();
    assert_eq!((parsed.value.milliseconds(), parsed.consumed), (0, 10));
    let offset = DatePolicy {
        default_offset_minutes: -300,
        ..policy
    };
    assert_eq!(
        offset
            .parse("1970-01-01", &NoNamedZones)
            .unwrap()
            .value
            .milliseconds(),
        18000000
    );
    assert_eq!(
        offset.format(DateValue::new(0)).unwrap(),
        "1969-12-31T19:00:00.000-0500"
    );
}

#[test]
fn named_zone_adapter_is_checked_and_failures_do_not_silently_fallback() {
    use intersect_model::definitions::scalars::date::{
        Calendar, DateFields, DatePolicy, ZoneResolver,
    };
    struct Resolver {
        offset: i32,
        consumed: usize,
        fail: bool,
    }
    impl ZoneResolver for Resolver {
        fn resolve(
            &self,
            suffix: &str,
            local: DateFields,
            calendar: Calendar,
        ) -> Result<Option<(i32, usize)>, String> {
            assert_eq!(suffix, "TEST");
            assert_eq!(local.year, 1970);
            assert_eq!(calendar, Calendar::ProlepticGregorian);
            if self.fail {
                return Err("Explicit DST ambiguity".into());
            }
            Ok(Some((self.offset, self.consumed)))
        }
    }
    let policy = DatePolicy {
        calendar: Calendar::ProlepticGregorian,
        default_offset_minutes: 0,
        require_full_input: false,
    };
    let text = "1970-01-01T00:00:00.000TEST";
    let resolver = Resolver {
        offset: 60,
        consumed: 4,
        fail: false,
    };
    assert_eq!(
        policy.parse(text, &resolver).unwrap().value.milliseconds(),
        -3600000
    );
    for invalid in [
        Resolver {
            offset: 1440,
            ..resolver
        },
        Resolver {
            consumed: 0,
            ..resolver
        },
        Resolver {
            consumed: 5,
            ..resolver
        },
        Resolver {
            fail: true,
            ..resolver
        },
    ] {
        assert!(policy.parse(text, &invalid).is_err());
    }
}

#[test]
fn date_formatter_has_explicit_bce_era_loss_and_ad_round_trips() {
    use intersect_model::definitions::scalars::date::{
        Calendar, DateFields, DatePolicy, NoNamedZones,
    };
    let policy = DatePolicy {
        calendar: Calendar::JulianGregorian1582,
        default_offset_minutes: 0,
        require_full_input: true,
    };
    for value in [
        DateValue::new(0),
        DateValue::new(-1),
        DateValue::new(i64::MAX),
        DateValue::new(-12219292800000),
    ] {
        assert_eq!(
            policy
                .parse(&policy.format(value).unwrap(), &NoNamedZones)
                .unwrap()
                .value,
            value
        );
    }
    let bce = DateValue::from_fields(
        DateFields {
            year: 0,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
            millisecond: 0,
        },
        policy.calendar,
        0,
    )
    .unwrap();
    let formatted = policy.format(bce).unwrap();
    assert_eq!(formatted, "0001-01-01T00:00:00.000+0000");
    assert_ne!(policy.parse(&formatted, &NoNamedZones).unwrap().value, bce);
    let restored: DateValue = serde_json::from_value(serde_json::to_value(bce).unwrap()).unwrap();
    assert_eq!(restored, bce);
}
