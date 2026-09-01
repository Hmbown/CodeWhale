use super::*;

const FIXTURES: &str = include_str!("fixtures/cases.json");

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FixtureFile {
    cases: Vec<FixtureCase>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FixtureCase {
    id: String,
    expect: String,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    accepted_seconds: Option<u64>,
    #[serde(default)]
    standard_equivalent_seconds: Option<u64>,
    #[serde(default)]
    admission: Option<ComputerAdmissionRequest>,
    #[serde(default)]
    observation: Option<ProviderObservation>,
    #[serde(default)]
    restated_observation: Option<ProviderObservation>,
    #[serde(default)]
    intervals: Vec<FixtureInterval>,
    #[serde(default)]
    profile_input: Option<String>,
    #[serde(default)]
    profile_id: Option<String>,
    #[serde(default)]
    multiplier: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FixtureInterval {
    admission: ComputerAdmissionRequest,
    observation: ProviderObservation,
}

fn fixtures() -> Vec<FixtureCase> {
    serde_json::from_str::<FixtureFile>(FIXTURES)
        .expect("computer meter fixtures must parse")
        .cases
}

fn issue_case(case: &FixtureCase) -> Result<ComputerMeterReceipt, ComputerMeterError> {
    let admission = bind_computer_admission(
        case.admission
            .clone()
            .expect("receipt fixture requires admission"),
    )?;
    issue_computer_meter_receipt(
        &admission,
        case.observation
            .clone()
            .expect("receipt fixture requires observation"),
    )
}

#[test]
fn fixtures_cover_provider_accepted_active_seconds_only() {
    let cases = fixtures();
    assert!(
        cases.len() >= 12,
        "fixture pack must cover accept, idle, queued, stopped, teardown, mismatch, concurrent, historic, correction"
    );

    for case in &cases {
        match case.expect.as_str() {
            "receipt" => {
                let receipt = issue_case(case).unwrap_or_else(|error| {
                    panic!("{} should mint a receipt, got {}", case.id, error.code())
                });
                assert_eq!(receipt.schema_id, COMPUTER_METER_RECEIPT_SCHEMA);
                assert_eq!(receipt.kind, COMPUTER_METER_RECEIPT_KIND);
                assert_eq!(receipt.accepted_seconds, case.accepted_seconds.unwrap());
                assert_eq!(
                    receipt.standard_equivalent_seconds,
                    case.standard_equivalent_seconds.unwrap()
                );
                assert_eq!(
                    receipt.standard_equivalent_seconds,
                    receipt.accepted_seconds * u64::from(receipt.multiplier)
                );
                let admission = bind_computer_admission(case.admission.clone().unwrap()).unwrap();
                assert_eq!(receipt.admission_id, admission.admission_id);
                assert_eq!(receipt.account_id, admission.account_id);
                assert_eq!(receipt.profile_id, admission.profile_id);
                assert_eq!(receipt.multiplier, admission.multiplier);
                assert_eq!(receipt.provider_allocation.cpu, admission.cpu);
                assert_eq!(receipt.provider_allocation.memory_gib, admission.memory_gib);
                assert_eq!(receipt.provider_allocation.disk_gib, admission.disk_gib);
                assert!(receipt.provider_allocation.accepted);
                assert_eq!(receipt.meter_revision, COMPUTER_METER_REVISION);
                assert_eq!(receipt.catalog_revision, COMPUTER_CATALOG_REVISION);
                assert_ne!(receipt.binding_digest, admission.binding_digest);
            }
            "error" => {
                let error = issue_case(case).expect_err(&case.id);
                assert_eq!(error.code(), case.code.as_deref().unwrap(), "{}", case.id);
            }
            "sum" => {
                let receipts: Vec<_> = case
                    .intervals
                    .iter()
                    .map(|interval| {
                        let admission = bind_computer_admission(interval.admission.clone())
                            .unwrap_or_else(|error| panic!("{}: {}", case.id, error));
                        issue_computer_meter_receipt(&admission, interval.observation.clone())
                            .unwrap_or_else(|error| panic!("{}: {}", case.id, error))
                    })
                    .collect();
                assert_eq!(
                    sum_standard_equivalent_seconds(&receipts),
                    case.standard_equivalent_seconds.unwrap()
                );
            }
            "decode" => {
                let profile = decode_computer_profile(case.profile_input.as_deref().unwrap())
                    .unwrap_or_else(|error| panic!("{}: {}", case.id, error));
                assert_eq!(profile.id.as_str(), case.profile_id.as_deref().unwrap());
                assert_eq!(profile.multiplier, case.multiplier.unwrap());
            }
            "admit_error" => {
                let error = admit_computer_profile(case.profile_input.as_deref().unwrap())
                    .expect_err(&case.id);
                assert_eq!(error.code(), case.code.as_deref().unwrap());
            }
            "correction" => {
                let admission = bind_computer_admission(case.admission.clone().unwrap()).unwrap();
                let original =
                    issue_computer_meter_receipt(&admission, case.observation.clone().unwrap())
                        .unwrap();
                let original_clone = original.clone();
                let correction = correct_computer_meter_receipt(
                    &original,
                    &admission,
                    case.restated_observation.clone().unwrap(),
                )
                .unwrap();
                assert_eq!(
                    original, original_clone,
                    "corrections must not mutate the original receipt"
                );
                assert_eq!(
                    correction.correction_of.as_deref(),
                    Some(original.receipt_id.as_str())
                );
                assert!(correction.lineage.contains(&original.receipt_id));
                assert_ne!(correction.receipt_id, original.receipt_id);
                assert_eq!(correction.accepted_seconds, case.accepted_seconds.unwrap());
                assert_eq!(
                    correction.standard_equivalent_seconds,
                    case.standard_equivalent_seconds.unwrap()
                );
                assert_eq!(correction.account_id, original.account_id);
                assert_eq!(correction.admission_id, original.admission_id);
                assert_eq!(correction.profile_id, original.profile_id);
            }
            other => panic!("unknown fixture expect `{other}`"),
        }
    }
}

#[test]
fn exact_replay_is_idempotent_and_a_mutated_binding_conflicts() {
    let case = fixtures()
        .into_iter()
        .find(|case| case.id == "accepted-standard-8")
        .unwrap();
    let first = issue_case(&case).unwrap();
    let replay = issue_case(&case).unwrap();
    assert_eq!(first.receipt_id, replay.receipt_id);
    assert_eq!(first.binding_digest, replay.binding_digest);
    assert_computer_meter_receipt_replay(&first, &replay).unwrap();

    let mut mutated = replay.clone();
    mutated.accepted_seconds = first.accepted_seconds + 1;
    mutated.standard_equivalent_seconds = mutated.accepted_seconds;
    assert_eq!(
        assert_computer_meter_receipt_replay(&first, &mutated)
            .unwrap_err()
            .code(),
        "computer_meter_receipt_replay_conflict"
    );
}

#[test]
fn admission_binds_profile_multiplier_account_and_expiry_before_dispatch() {
    let admission = bind_computer_admission(ComputerAdmissionRequest {
        admission_id: "adm_bind".to_string(),
        account_id: "acct_demo".to_string(),
        computer_id: "cmp_bind".to_string(),
        run_id: "run_bind".to_string(),
        provider: "daytona".to_string(),
        profile_id: "standard-16".to_string(),
        funding_authority: "coding_membership_included".to_string(),
        quote_id: "quote_bind".to_string(),
        expires_at: "2026-08-31T18:00:00.000Z".to_string(),
        meter_revision: String::new(),
        catalog_revision: String::new(),
    })
    .unwrap();
    assert_eq!(admission.schema_id, COMPUTER_ADMISSION_SCHEMA);
    assert_eq!(admission.profile_id, ComputerProfileId::Standard16);
    assert_eq!(admission.cpu, 4);
    assert_eq!(admission.memory_gib, 16);
    assert_eq!(admission.disk_gib, 16);
    assert_eq!(admission.multiplier, 2);
    assert_eq!(admission.account_id, "acct_demo");
    assert_eq!(admission.meter_revision, COMPUTER_METER_REVISION);
    assert!(!admission.binding_digest.is_empty());
}

#[test]
fn one_standard_16_matches_two_standard_8_over_the_same_ten_minutes() {
    let cases = fixtures();
    let two_eights = cases
        .iter()
        .find(|case| case.id == "concurrent-two-standard-8")
        .unwrap();
    let sixteen = cases
        .iter()
        .find(|case| case.id == "accepted-standard-16")
        .unwrap();
    let eight_receipts: Vec<_> = two_eights
        .intervals
        .iter()
        .map(|interval| {
            let admission = bind_computer_admission(interval.admission.clone()).unwrap();
            issue_computer_meter_receipt(&admission, interval.observation.clone()).unwrap()
        })
        .collect();
    let sixteen_receipt = issue_case(sixteen).unwrap();
    assert_eq!(
        sum_standard_equivalent_seconds(&eight_receipts),
        sixteen_receipt.standard_equivalent_seconds
    );
    assert_eq!(sum_standard_equivalent_seconds(&eight_receipts), 1200);
}
