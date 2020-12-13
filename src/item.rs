//Copyright 2016 secret-service-rs Developers
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use error::SsError;
use item_proxy::ItemInterfaceProxy;
use session::Session;
use ss::{
    SS_DBUS_NAME,
    SS_INTERFACE_SERVICE,
    SS_PATH,
};
use ss_crypto::decrypt;
use util::{
    exec_prompt,
    format_secret_zbus,
    Interface,
};

use dbus::{
    BusName,
    Connection,
    MessageItem,
    Path,
};
use dbus::MessageItem::{
    Array,
    ObjectPath,
};
use dbus::Interface as InterfaceName;
use std::collections::HashMap;
use std::convert::{TryFrom, TryInto};
use std::rc::Rc;

// Helper enum for locking
enum LockAction {
    Lock,
    Unlock,
}

pub struct Item<'a> {
    // TODO: Implement method for path?
    bus: Rc<Connection>,
    zbus: Rc<zbus::Connection>,
    session: &'a Session,
    pub item_path: Path,
    // TODO currently instantiating on demand because of lifetime issues on item_path
    //item_interface: ItemInterfaceProxy<'a>,
    service_interface: Interface,
}

impl<'a> Item<'a> {
    pub fn new(bus: Rc<Connection>,
               zbus: Rc<zbus::Connection>,
               session: &'a Session,
               item_path: Path
               ) -> Self {
        let service_interface = Interface::new(
            bus.clone(),
            BusName::new(SS_DBUS_NAME).unwrap(),
            Path::new(SS_PATH).unwrap(),
            InterfaceName::new(SS_INTERFACE_SERVICE).unwrap()
        );
        Item {
            bus,
            zbus,
            session,
            item_path,
            service_interface,
        }
    }

    pub fn is_locked(&self) -> ::Result<bool> {
        let item_interface = ItemInterfaceProxy::new_for(
            &self.zbus,
            SS_DBUS_NAME,
            &self.item_path[..],
            )
            .unwrap();
        Ok(item_interface.locked()?)
    }

    pub fn ensure_unlocked(&self) -> ::Result<()> {
        if self.is_locked()? {
            Err(SsError::Locked)
        } else {
            Ok(())
        }
    }

    //Helper function for locking and unlocking
    // TODO: refactor into utils? It should be same as collection
    fn lock_or_unlock(&self, lock_action: LockAction) -> ::Result<()> {
        let objects = MessageItem::new_array(
            vec![ObjectPath(self.item_path.clone())]
        ).unwrap();

        let lock_action_str = match lock_action {
            LockAction::Lock => "Lock",
            LockAction::Unlock => "Unlock",
        };

        let res = self.service_interface.method(lock_action_str, vec![objects])?;
        if let Some(&Array(ref unlocked, _)) = res.get(0) {
            if unlocked.is_empty() {
                if let Some(&ObjectPath(ref path)) = res.get(1) {
                    exec_prompt(self.bus.clone(), path.clone())?;
                }
            }
        }
        Ok(())
    }

    pub fn unlock(&self) -> ::Result<()> {
        self.lock_or_unlock(LockAction::Unlock)
    }

    pub fn lock(&self) -> ::Result<()> {
        self.lock_or_unlock(LockAction::Lock)
    }

    pub fn get_attributes(&self) -> ::Result<Vec<(String, String)>> {
        let item_interface = ItemInterfaceProxy::new_for(
            &self.zbus,
            SS_DBUS_NAME,
            &self.item_path[..],
            )
            .unwrap();
        let attributes = item_interface.attributes()?;
        let attributes: HashMap<String, String> = attributes.try_into().map_err(|_| SsError::Parse)?;

        let res = attributes.iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<Vec<(String, String)>>();

        Ok(res)
    }

    // Probably best example of creating dict
    pub fn set_attributes(&self, attributes: Vec<(&str, &str)>) -> ::Result<()> {
        let item_interface = ItemInterfaceProxy::new_for(
            &self.zbus,
            SS_DBUS_NAME,
            &self.item_path[..],
            )
            .unwrap();
        if !attributes.is_empty() {
            let attributes: HashMap<&str, &str> = attributes.into_iter().collect();
            Ok(item_interface.set_attributes(attributes.into())?)
        } else {
            Ok(())
        }
    }

    pub fn get_label(&self) -> ::Result<String> {
        let item_interface = ItemInterfaceProxy::new_for(
            &self.zbus,
            SS_DBUS_NAME,
            &self.item_path[..],
            )
            .unwrap();
        Ok(item_interface.label()?)
    }

    pub fn set_label(&self, new_label: &str) -> ::Result<()> {
        let item_interface = ItemInterfaceProxy::new_for(
            &self.zbus,
            SS_DBUS_NAME,
            &self.item_path[..],
            )
            .unwrap();
        Ok(item_interface.set_label(new_label)?)
    }

    /// Deletes dbus object, but struct instance still exists (current implementation)
    pub fn delete(&self) -> ::Result<()> {
        let item_interface = ItemInterfaceProxy::new_for(
            &self.zbus,
            SS_DBUS_NAME,
            &self.item_path[..],
            )
            .unwrap();
        //Because of ensure_unlocked, no prompt is really necessary
        //basically,you must explicitly unlock first
        self.ensure_unlocked()?;
        let prompt_path = item_interface.delete()?;

        if prompt_path != "/" {
                exec_prompt(self.bus.clone(), dbus::Path::new(prompt_path.clone()).unwrap())?;
        } else {
            return Ok(());
        }
        // If for some reason the patterns don't match, return error
        Err(SsError::Parse)
    }

    pub fn get_secret(&self) -> ::Result<Vec<u8>> {
        let item_interface = ItemInterfaceProxy::new_for(
            &self.zbus,
            SS_DBUS_NAME,
            &self.item_path[..],
            )
            .unwrap();
        let session = zvariant::ObjectPath::try_from(self.session.object_path.to_string()).expect("remove this expect later");
        dbg!(session.to_string());
        let secret_struct = item_interface.get_secret(session)?;
        dbg!("hit");
        let secret = secret_struct.value;

        if !self.session.is_encrypted() {
            Ok(secret)
        } else {
            // get "param" (aes_iv) field out of secret struct
            let aes_iv = secret_struct.parameters;

            // decrypt
            let decrypted_secret = decrypt(&secret[..], &self.session.get_aes_key()[..], &aes_iv[..]).unwrap();

            Ok(decrypted_secret)
        }
    }

    pub fn get_secret_content_type(&self) -> ::Result<String> {
        let item_interface = ItemInterfaceProxy::new_for(
            &self.zbus,
            SS_DBUS_NAME,
            &self.item_path[..],
            )
            .unwrap();
        let session = zvariant::ObjectPath::try_from(self.session.object_path.to_string()).expect("remove this expect later");
        let secret_struct = item_interface.get_secret(session)?;
        let content_type = secret_struct.content_type;

        Ok(content_type.clone())
    }

    pub fn set_secret(&self, secret: &[u8], content_type: &str) -> ::Result<()> {
        let item_interface = ItemInterfaceProxy::new_for(
            &self.zbus,
            SS_DBUS_NAME,
            &self.item_path[..],
            )
            .unwrap();
        let secret_struct = format_secret_zbus(&self.session, secret, content_type)?;
        Ok(item_interface.set_secret(secret_struct)?)
    }

    pub fn get_created(&self) -> ::Result<u64> {
        let item_interface = ItemInterfaceProxy::new_for(
            &self.zbus,
            SS_DBUS_NAME,
            &self.item_path[..],
            )
            .unwrap();
        Ok(item_interface.created()?)
    }

    pub fn get_modified(&self) -> ::Result<u64> {
        let item_interface = ItemInterfaceProxy::new_for(
            &self.zbus,
            SS_DBUS_NAME,
            &self.item_path[..],
            )
            .unwrap();
        Ok(item_interface.modified()?)
    }
}

impl<'a> Eq for Item<'a> {}
impl<'a> PartialEq for Item<'a> {
    fn eq(&self, other: &Item) -> bool {
        self.item_path == other.item_path &&
        self.get_attributes().unwrap() == other.get_attributes().unwrap()
    }
}

#[cfg(test)]
mod test{
    use super::super::*;

    #[test]
    fn should_create_and_delete_item() {
        let ss = SecretService::new(EncryptionType::Plain).unwrap();
        let collection = ss.get_default_collection().unwrap();
        let item = collection.create_item(
            "Test",
            Vec::new(),
            b"test",
            false, // replace
            "text/plain" // content_type
        ).unwrap();
        let _ = item.item_path.clone(); // to prepare for future drop for delete?
        item.delete().unwrap();
        // Random operation to prove that path no longer exists
        match item.get_label() {
            Ok(_) => panic!(),
            Err(_) => (),
        }
    }

    #[test]
    fn should_check_if_item_locked() {
        let ss = SecretService::new(EncryptionType::Plain).unwrap();
        let collection = ss.get_default_collection().unwrap();
        let item = collection.create_item(
            "Test",
            Vec::new(),
            b"test",
            false, // replace
            "text/plain" // content_type
        ).unwrap();
        item.is_locked().unwrap();
        item.delete().unwrap();
    }

    #[test]
    #[ignore]
    fn should_lock_and_unlock() {
        let ss = SecretService::new(EncryptionType::Plain).unwrap();
        let collection = ss.get_default_collection().unwrap();
        let item = collection.create_item(
            "Test",
            Vec::new(),
            b"test",
            false, // replace
            "text/plain" // content_type
        ).unwrap();
        let locked = item.is_locked().unwrap();
        if locked {
            item.unlock().unwrap();
            item.ensure_unlocked().unwrap();
            assert!(!item.is_locked().unwrap());
            item.lock().unwrap();
            assert!(item.is_locked().unwrap());
        } else {
            item.lock().unwrap();
            assert!(item.is_locked().unwrap());
            item.unlock().unwrap();
            item.ensure_unlocked().unwrap();
            assert!(!item.is_locked().unwrap());
        }
        item.delete().unwrap();
    }

    #[test]
    fn should_get_and_set_item_label() {
        let ss = SecretService::new(EncryptionType::Plain).unwrap();
        let collection = ss.get_default_collection().unwrap();
        let item = collection.create_item(
            "Test",
            Vec::new(),
            b"test",
            false, // replace
            "text/plain" // content_type
        ).unwrap();

        // Set label to test and check
        item.set_label("Tester").unwrap();
        let label = item.get_label().unwrap();
        assert_eq!(label, "Tester");
        println!("{:?}", label);
        item.delete().unwrap();
        //assert!(false);
    }

    #[test]
    fn should_create_with_item_attributes() {
        let ss = SecretService::new(EncryptionType::Plain).unwrap();
        let collection = ss.get_default_collection().unwrap();
        let item = collection.create_item(
            "Test",
            vec![("test_attributes_in_item", "test")],
            b"test",
            false, // replace
            "text/plain" // content_type
        ).unwrap();
        let attributes = item.get_attributes().unwrap();
        assert_eq!(attributes, vec![("test_attributes_in_item".into(), "test".into())]);
        println!("Attributes: {:?}", attributes);
        item.delete().unwrap();
        //assert!(false);
    }

    #[test]
    fn should_get_and_set_item_attributes() {
        let ss = SecretService::new(EncryptionType::Plain).unwrap();
        let collection = ss.get_default_collection().unwrap();
        let item = collection.create_item(
            "Test",
            Vec::new(),
            b"test",
            false, // replace
            "text/plain" // content_type
        ).unwrap();
        // Also test empty array handling
        item.set_attributes(vec![]).unwrap();
        item.set_attributes(vec![("test_attributes_in_item_get", "test")]).unwrap();
        let attributes = item.get_attributes().unwrap();
        println!("Attributes: {:?}", attributes);
        assert_eq!(attributes, vec![("test_attributes_in_item_get".into(), "test".into())]);
        item.delete().unwrap();
        //assert!(false);
    }
    #[test]
    fn should_get_modified_created_props() {
        let ss = SecretService::new(EncryptionType::Plain).unwrap();
        let collection = ss.get_default_collection().unwrap();
        let item = collection.create_item(
            "Test",
            Vec::new(),
            b"test",
            false, // replace
            "text/plain" // content_type
        ).unwrap();
        item.set_label("Tester").unwrap();
        let created = item.get_created().unwrap();
        let modified = item.get_modified().unwrap();
        println!("Created {:?}, Modified {:?}", created, modified);
        item.delete().unwrap();
        //assert!(false);
    }

    #[test]
    fn should_create_and_get_secret() {
        let ss = SecretService::new(EncryptionType::Plain).unwrap();
        let collection = ss.get_default_collection().unwrap();
        let item = collection.create_item(
            "Test",
            Vec::new(),
            b"test",
            false, // replace
            "text/plain" // content_type
        ).unwrap();
        let secret = item.get_secret().unwrap();
        item.delete().unwrap();
        assert_eq!(secret, b"test");
    }

    #[test]
    fn should_create_and_get_secret_encrypted() {
        let ss = SecretService::new(EncryptionType::Dh).unwrap();
        let collection = ss.get_default_collection().unwrap();
        let item = collection.create_item(
            "Test",
            Vec::new(),
            b"test",
            false, // replace
            "text/plain" // content_type
        ).unwrap();
        let secret = item.get_secret().unwrap();
        item.delete().unwrap();
        assert_eq!(secret, b"test");
    }

    #[test]
    fn should_get_secret_content_type() {
        let ss = SecretService::new(EncryptionType::Plain).unwrap();
        let collection = ss.get_default_collection().unwrap();
        let item = collection.create_item(
            "Test",
            Vec::new(),
            b"test",
            false, // replace
            "text/plain" // content_type, defaults to text/plain
        ).unwrap();
        let content_type = item.get_secret_content_type().unwrap();
        item.delete().unwrap();
        assert_eq!(content_type, "text/plain".to_owned());
    }

    #[test]
    fn should_set_secret() {
        let ss = SecretService::new(EncryptionType::Plain).unwrap();
        let collection = ss.get_default_collection().unwrap();
        let item = collection.create_item(
            "Test",
            Vec::new(),
            b"test",
            false, // replace
            "text/plain" // content_type
        ).unwrap();
        item.set_secret(b"new_test", "text/plain").unwrap();
        let secret = item.get_secret().unwrap();
        item.delete().unwrap();
        assert_eq!(secret, b"new_test");
    }

    #[test]
    fn should_create_encrypted_item() {
        let ss = SecretService::new(EncryptionType::Dh).unwrap();
        let collection = ss.get_default_collection().unwrap();
        let item = collection.create_item(
            "Test",
            Vec::new(),
            b"test_encrypted",
            false, // replace
            "text/plain" // content_type
        ).expect("Error on item creation");
        let secret = item.get_secret().unwrap();
        item.delete().unwrap();
        assert_eq!(secret, b"test_encrypted");
    }

    #[test]
    fn should_create_encrypted_item_from_empty_secret() {
        //empty string
        let ss = SecretService::new(EncryptionType::Dh).unwrap();
        let collection = ss.get_default_collection().unwrap();
        let item = collection.create_item(
            "Test",
            Vec::new(),
            b"",
            false, // replace
            "text/plain" // content_type
        ).expect("Error on item creation");
        let secret = item.get_secret().unwrap();
        item.delete().unwrap();
        assert_eq!(secret, b"");
    }

    #[test]
    fn should_get_encrypted_secret_across_dbus_connections() {
        {
            let ss = SecretService::new(EncryptionType::Dh).unwrap();
            let collection = ss.get_default_collection().unwrap();
            let item = collection.create_item(
                "Test",
                vec![("test_attributes_in_item_encrypt", "test")],
                b"test_encrypted",
                false, // replace
                "text/plain" // content_type
            ).expect("Error on item creation");
            let secret = item.get_secret().unwrap();
            assert_eq!(secret, b"test_encrypted");
        }
        {
            let ss = SecretService::new(EncryptionType::Dh).unwrap();
            let collection = ss.get_default_collection().unwrap();
            let search_item = collection.search_items(
                vec![("test_attributes_in_item_encrypt", "test")]
            ).unwrap();
            let item = search_item.get(0).unwrap().clone();
            assert_eq!(item.get_secret().unwrap(), b"test_encrypted");
            item.delete().unwrap();
        }
    }
}

