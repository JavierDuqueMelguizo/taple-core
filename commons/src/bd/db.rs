use std::{
    collections::{HashMap, HashSet},
    path::Path,
    str::FromStr,
};

use crate::{
    errors::SubjectError,
    identifier::{Derivable, DigestIdentifier},
    models::{
        event::Event,
        event_content::EventContent,
        event_request::EventRequest,
        signature::Signature,
        state::{LedgerState, Subject},
    },
};

use super::{
    level_db::{
        error::WrapperLevelDBErrors,
        wrapper_leveldb::{CursorIndex, StringKey, WrapperLevelDB},
    },
    TapleDB,
};

const SIGNATURE_TABLE: &str = "signature";
const SUBJECT_TABLE: &str = "subject";
const EVENT_TABLE: &str = "event";
const REQUEST_TABLE: &str = "request";
const ID_TABLE: &str = "controller-id";

pub struct DB {
    signature_db: WrapperLevelDB<StringKey, HashSet<Signature>>,
    subject_db: WrapperLevelDB<StringKey, Subject>,
    event_db: WrapperLevelDB<StringKey, Event>,
    request_db: WrapperLevelDB<StringKey, EventRequest>,
    id_db: WrapperLevelDB<StringKey, String>,
}

impl DB {
    pub fn new(db: std::sync::Arc<leveldb::database::Database<StringKey>>) -> Self {
        Self {
            signature_db: WrapperLevelDB::<StringKey, HashSet<Signature>>::new(
                db.clone(),
                SIGNATURE_TABLE,
            ),
            subject_db: WrapperLevelDB::<StringKey, Subject>::new(db.clone(), SUBJECT_TABLE),
            event_db: WrapperLevelDB::<StringKey, Event>::new(db.clone(), EVENT_TABLE),
            request_db: WrapperLevelDB::<StringKey, EventRequest>::new(db.clone(), REQUEST_TABLE),
            id_db: WrapperLevelDB::<StringKey, String>::new(db.clone(), ID_TABLE),
        }
    }
}

impl DB {
    fn _get_subject(&self, subject_id: &DigestIdentifier) -> Result<Subject, WrapperLevelDBErrors> {
        let id = subject_id.to_str();
        self.subject_db.get(&id)
    }
}

impl TapleDB for DB {
    fn get_controller_id(&self) -> Option<String> {
        match self.id_db.get("") {
            Ok(id) => Some(id),
            Err(WrapperLevelDBErrors::EntryNotFoundError) => None,
            Err(error) => {
                panic!("Not recoverable error get_controller_id {:?}", error);
            }
        }
    }

    fn set_controller_id(&self, controller_id: String) {
        if let Err(error) = self.id_db.put("", controller_id) {
            panic!("Error while inserting controller_id. Error --> {}", error);
        }
    }

    fn get_event(&self, subject_id: &DigestIdentifier, sn: u64) -> Option<Event> {
        let id = subject_id.to_str();
        let events_by_subject = self.event_db.partition(&id);
        match events_by_subject.get(&sn.to_string()) {
            Ok(event) => Some(event),
            Err(error) => match error {
                WrapperLevelDBErrors::EntryNotFoundError => None,
                _ => {
                    println!("ERRORR: {:?}", error);
                    panic!("Not recoverable error get event")},
            },
        }
    }

    fn get_events_by_range(
        &self,
        subject_id: &DigestIdentifier,
        from: Option<String>,
        quantity: isize,
    ) -> Vec<Event> {
        let id = subject_id.to_str();
        let events_by_subject = self.event_db.partition(&id);
        let cursor = match from {
            Some(value) => CursorIndex::FromKey(value),
            None => {
                if quantity < 0 {
                    CursorIndex::FromEnding
                } else {
                    CursorIndex::FromBeginning
                }
            }
        };
        let quantity = match cursor {
            CursorIndex::FromBeginning => quantity.abs(),
            CursorIndex::FromEnding => {
                if quantity > 0 {
                    quantity * -1
                } else {
                    quantity
                }
            }
            CursorIndex::FromKey(_) => quantity,
        };
        events_by_subject
            .get_range(&cursor, quantity)
            .into_iter()
            .map(|x| x.1)
            .collect()
    }

    fn set_event(&self, subject_id: &DigestIdentifier, event: Event) {
        let id = subject_id.to_str();
        let events_by_subject = self.event_db.partition(&id);
        let sn = event.event_content.sn.to_string();
        if let Err(error) = events_by_subject.put(&sn, event) {
            panic!(
                "Error while inserting event sn:{} on subject_id:[{}]. Error --> {}",
                sn, id, error
            );
        }
    }

    fn get_signatures(&self, subject_id: &DigestIdentifier, sn: u64) -> Option<HashSet<Signature>> {
        let id = subject_id.to_str();
        let signatures_by_subject = self.signature_db.partition(&id);
        match signatures_by_subject.get(&sn.to_string()) {
            Ok(signatures) => Some(signatures),
            Err(error) => match error {
                WrapperLevelDBErrors::EntryNotFoundError => None,
                _ => panic!("Not recoverable error get signatures"),
            },
        }
    }

    fn set_signatures(
        &self,
        subject_id: &DigestIdentifier,
        sn: u64,
        signatures: HashSet<Signature>,
    ) {
        let id = subject_id.to_str();
        let signatures_by_subject = self.signature_db.partition(&id);
        let sn = sn.to_string();
        let total_signatures = match signatures_by_subject.get(&sn.to_string()) {
            Ok(other) => signatures.union(&other).cloned().collect(),
            Err(error) => match error {
                WrapperLevelDBErrors::EntryNotFoundError => signatures,
                _ => panic!("Not recoverable error get signatures"),
            },
        };
        if let Err(error) = signatures_by_subject.put(&sn.to_string(), total_signatures) {
            panic!(
                "Error while inserting event sn:{} on subject_id:[{}]. Error --> {}",
                sn, id, error
            );
        }
    }

    fn get_subject(&self, subject_id: &DigestIdentifier) -> Option<Subject> {
        match self._get_subject(subject_id) {
            Ok(subject) => Some(subject),
            Err(error) => match error {
                WrapperLevelDBErrors::EntryNotFoundError => None,
                _ => panic!("Not recoverable error get subject"),
            },
        }
    }

    fn set_subject(&self, subject_id: &DigestIdentifier, subject: Subject) {
        let id = subject_id.to_str();
        if let Err(error) = self.subject_db.put(&id, subject) {
            panic!(
                "Error while inserting subject_id:[{}]. Error --> {}",
                id, error
            );
        }
    }

    fn apply_event_sourcing(&self, event_content: EventContent) -> Result<(), SubjectError> {
        let subject_id = event_content.subject_id.clone();
        let mut subject = self._get_subject(&subject_id).unwrap();
        subject.apply(event_content.clone())?;
        // Persist the change
        self.set_subject(&subject_id, subject);
        let id = subject_id.to_str();
        let signatures_by_subject = self.signature_db.partition(&id);
        match signatures_by_subject.del(&(event_content.sn - 1).to_string()) {
            Ok(_) => Ok(()),
            Err(error) => match error {
                WrapperLevelDBErrors::EntryNotFoundError => Ok(()),
                _ => Err(SubjectError::DeleteSignaturesFailed),
            },
        }
    }

    fn get_all_heads(&self) -> HashMap<DigestIdentifier, LedgerState> {
        let mut result = HashMap::new();
        for (key, subject) in self.subject_db.get_all().iter() {
            let subject_id = DigestIdentifier::from_str(&key.0).expect("La conversion va bien");
            result.insert(subject_id, subject.ledger_state.to_owned());
        }
        result
    }

    fn set_negociating_true(&self, subject_id: &DigestIdentifier) -> Result<(), SubjectError> {
        let mut subject = match self._get_subject(subject_id) {
            Ok(subject) => subject,
            Err(error) => match error {
                WrapperLevelDBErrors::EntryNotFoundError => {
                    return Err(SubjectError::SubjectNotFound)
                }
                _ => panic!("Not recoverable error get subject"),
            },
        };
        subject.ledger_state.negociating_next = true;
        // Persist the change
        self.set_subject(&subject_id, subject);
        Ok(())
    }

    fn get_all_subjects(&self) -> Vec<Subject> {
        let mut result = Vec::new();
        for (_, subject) in self.subject_db.get_all().iter() {
            result.push(subject.to_owned());
        }
        result
    }

    fn get_all_request(&self) -> Vec<EventRequest> {
        let mut result = Vec::new();
        for (_, request) in self.request_db.get_all().iter() {
            result.push(request.to_owned());
        }
        result
    }

    fn get_request(
        &self,
        subject_id: &DigestIdentifier,
        request_id: &DigestIdentifier,
    ) -> Option<EventRequest> {
        let id = subject_id.to_str();
        let requests_by_subject = self.request_db.partition(&id);
        match requests_by_subject.get(&request_id.to_str()) {
            Ok(request) => Some(request),
            Err(error) => match error {
                WrapperLevelDBErrors::EntryNotFoundError => None,
                _ => panic!("Not recoverable error get request"),
            },
        }
    }

    fn del_request(
        &self,
        subject_id: &DigestIdentifier,
        request_id: &DigestIdentifier,
    ) -> Option<EventRequest> {
        let id = subject_id.to_str();
        let requests_by_subject = self.request_db.partition(&id);
        match requests_by_subject.del(&request_id.to_str()) {
            Ok(request) => request,
            Err(error) => match error {
                WrapperLevelDBErrors::EntryNotFoundError => None,
                _ => panic!("Not recoverable error get request"),
            },
        }
    }

    fn set_request(&self, subject_id: &DigestIdentifier, request: EventRequest) {
        let id = subject_id.to_str();
        let requests_by_subject = self.request_db.partition(&id);
        let req_id = request.signature.content.event_content_hash.to_str();
        if let Err(error) = requests_by_subject.put(&req_id, request) {
            panic!(
                "Error while inserting request id:{} on subject_id:[{}]. Error --> {}",
                req_id, id, error
            );
        }
    }
}

use leveldb::options::Options as LevelDBOptions;

pub fn open_db(path: &Path) -> std::sync::Arc<leveldb::database::Database<StringKey>> {
    let mut db_options = LevelDBOptions::new();
    db_options.create_if_missing = true;

    if let Ok(db) = crate::bd::level_db::wrapper_leveldb::open_db(path, db_options) {
        std::sync::Arc::new(db)
    } else {
        panic!("Could not open LevelDB connection")
    }
}

#[cfg(test)]
mod tests {

    use std::str::FromStr;

    use tempdir::TempDir;
    use tokio::runtime::Runtime;

    use crate::{bd::TapleDB, identifier::DigestIdentifier, models::event::Event};

    use super::{open_db, DB};

    #[test]
    fn test_simple_insert() {
        let rt = Runtime::new().unwrap();
        rt.block_on(async {
            // Generated a temporary directory for this test...
            let temp_dir = TempDir::new("test_simple_insert").unwrap();
            let subject_id = DigestIdentifier::default();
            let event = Event::default();
            {
                // Open connection...
                let db = DB::new(open_db(temp_dir.path()));
                // Insert an event...
                db.set_event(&subject_id, event.clone())
            }
            {
                // We open it again
                let db = DB::new(open_db(temp_dir.path()));
                // Retrive the inserted event... (to check the persistence)
                let ev0 = db.get_event(&subject_id, 1);
                assert!(ev0.is_some());
                assert_eq!(ev0.unwrap(), event)
            }
        })
    }

    #[test]
    fn test_open_db() {
        // Generated a temporary directory for this test...
        let temp_dir = TempDir::new("test_simple_insert").unwrap();
        let pre_db = open_db(temp_dir.path());
        let db1 = DB::new(pre_db.clone());
        let db2 = DB::new(pre_db.clone());
        let _db3 = DB::new(pre_db.clone());
        let _db4 = DB::new(pre_db.clone());
        let subject_id = DigestIdentifier::from_str("Ju536BiUXBqbuNdJsOBwYWnbzrKjsYtVEauI6IsMh3tM").unwrap();
        let event = Event::default();
        db1.set_event(&subject_id, event.clone());
        assert_eq!(db2.get_event(&subject_id, 1).unwrap(), event);
    }
}
